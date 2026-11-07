//! Repository-backed agent skills with private master copies and safe per-app deployment.
mod files;
mod git;
mod transaction;

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use toolbox_core::skills::{
    InstalledSkill, LocalSkill, SkillApp, SkillAppPath, SkillCandidate, SkillRepository,
    SkillsCommand, SkillsSnapshot,
};
use transaction::Change;

const DAY: u64 = 24 * 60 * 60;
const RETRY: u64 = 60 * 60;
const MAX_SKILLS: usize = 250;
const MAX_REPOSITORIES: usize = 16;

#[derive(Clone, Serialize, Deserialize)]
struct ManagedSkill {
    skill: InstalledSkill,
    fingerprint: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct State {
    schema: u32,
    generation: u64,
    next_id: u64,
    repositories: Vec<SkillRepository>,
    installed: Vec<ManagedSkill>,
    daily_update: bool,
    last_attempt_at: Option<u64>,
    last_success_at: Option<u64>,
    next_update_at: Option<u64>,
    message: String,
    last_error: Option<String>,
    warnings: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: 1,
            generation: 0,
            next_id: 2,
            repositories: vec![SkillRepository {
                id: "repo-1".into(),
                url: "https://github.com/anthropics/skills.git".into(),
                branch: None,
                revision: None,
                last_error: None,
            }],
            installed: Vec::new(),
            daily_update: false,
            last_attempt_at: None,
            last_success_at: None,
            next_update_at: None,
            message: String::new(),
            last_error: None,
            warnings: Vec::new(),
        }
    }
}

/// The host runs this service on one worker; the file lock also excludes another host.
pub struct SkillsService {
    root: PathBuf,
    home: PathBuf,
    state: State,
    available: Vec<SkillCandidate>,
    local: Vec<LocalSkill>,
    caches: BTreeMap<String, git::Checkout>,
    fetcher: Box<dyn git::Fetcher>,
    cancel: Arc<AtomicBool>,
    _lock: fs::File,
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Canonical paths on Windows carry a verbatim prefix. Display-only paths
/// should look like the paths users type and see elsewhere.
fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    #[cfg(windows)]
    {
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{rest}");
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return rest.to_owned();
        }
    }
    text.into_owned()
}

impl SkillsService {
    pub fn open(root: PathBuf, home: PathBuf, cancel: Arc<AtomicBool>) -> Result<Self, String> {
        if !root.is_absolute() || !home.is_absolute() {
            return Err("Skills storage and home directories must be absolute".into());
        }
        if fs::symlink_metadata(&root).is_ok() {
            files::metadata(&root)?;
        }
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        let home = home
            .canonicalize()
            .map_err(|e| format!("Cannot locate home directory: {e}"))?;
        for directory in [root.join("managed"), root.join("temporary")] {
            files::checked_path(&root, &directory, true)?;
        }
        let lock_path = root.join("manager.lock");
        if files::exists(&lock_path)? && !files::metadata(&lock_path)?.is_file() {
            return Err("Skills manager lock is not a file".into());
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|_| "Another skills manager is running")?;
        let path = root.join("state.json");
        let state = if files::exists(&path)? {
            serde_json::from_slice(&files::read_limited(&path, 1024 * 1024)?)
                .map_err(|e| format!("Invalid skills state (file preserved): {e}"))?
        } else {
            State::default()
        };
        validate_state(&state)?;
        let mut service = Self {
            root,
            home,
            state,
            available: Vec::new(),
            local: Vec::new(),
            caches: BTreeMap::new(),
            fetcher: Box::new(git::GitFetcher),
            cancel,
            _lock: lock,
        };
        service.check_roots(false)?;
        // New startup findings take precedence over older persisted warnings
        // when the bounded list is full.
        let mut warnings = clear_stale_staging(&service.root)?;
        warnings.extend(transaction::recover(
            &service.root,
            &service.allowed_parents(),
            service.state.generation,
        )?);
        warnings.append(&mut service.state.warnings);
        service.state.warnings = warnings;
        service.state.warnings.truncate(16);
        Ok(service)
    }

    pub fn snapshot(&self) -> SkillsSnapshot {
        let mut snapshot = SkillsSnapshot {
            storage_path: display_path(&self.root.join("managed")),
            app_paths: SkillApp::ALL
                .into_iter()
                .map(|app| SkillAppPath {
                    app,
                    path: display_path(&self.app_root(app)),
                })
                .collect(),
            repositories: self.state.repositories.clone(),
            installed: self
                .state
                .installed
                .iter()
                .map(|entry| entry.skill.clone())
                .collect(),
            available: self
                .available
                .iter()
                .cloned()
                .map(|mut candidate| {
                    candidate.installed = self.state.installed.iter().any(|entry| {
                        entry.skill.repository_id.as_deref() == Some(&candidate.repository_id)
                            && entry.skill.source_path.as_deref() == Some(&candidate.path)
                    });
                    candidate
                })
                .collect(),
            local: self.local.clone(),
            daily_update: self.state.daily_update,
            last_attempt_at: self.state.last_attempt_at,
            last_success_at: self.state.last_success_at,
            next_update_at: self.next_due(),
            busy: false,
            message: self.state.message.clone(),
            last_error: self.state.last_error.clone(),
            warnings: self.state.warnings.clone(),
            truncated: false,
        };
        // Leave generous space for the host response envelope and status. An
        // unusually large catalog must not make IPC unusable for removing it.
        while serde_json::to_vec(&snapshot).is_ok_and(|bytes| bytes.len() > 700 * 1024) {
            snapshot.truncated = true;
            let rows = if !snapshot.available.is_empty() {
                &mut snapshot.available
            } else {
                // The lists have different row types; trim their lengths in
                // bounded chunks rather than repeatedly serializing each row.
                if !snapshot.local.is_empty() {
                    snapshot
                        .local
                        .truncate(snapshot.local.len().saturating_sub(32));
                    continue;
                }
                if !snapshot.installed.is_empty() {
                    snapshot
                        .installed
                        .truncate(snapshot.installed.len().saturating_sub(32));
                    continue;
                }
                break;
            };
            rows.truncate(rows.len().saturating_sub(32));
        }
        snapshot
    }

    pub fn next_due(&self) -> Option<u64> {
        self.state
            .daily_update
            .then_some(self.state.next_update_at.unwrap_or(0))
    }

    pub fn execute(&mut self, command: SkillsCommand, now: u64) -> Result<String, String> {
        let result = self.execute_inner(command, now);
        self.finish(result)
    }

    pub fn run_daily(&mut self, now: u64) -> Result<String, String> {
        if !self.next_due().is_some_and(|due| due <= now) {
            return Ok("Daily skills update is not due".into());
        }
        self.execute(SkillsCommand::Update { skill_id: None }, now)
    }

    fn execute_inner(&mut self, command: SkillsCommand, now: u64) -> Result<String, String> {
        self.check_cancel()?;
        self.check_roots(false)?;
        let warnings =
            transaction::recover(&self.root, &self.allowed_parents(), self.state.generation)?;
        self.state.warnings.splice(0..0, warnings);
        self.state.warnings.truncate(16);
        match command {
            SkillsCommand::AddRepository { url, branch } => self.add_repository(url, branch),
            SkillsCommand::RemoveRepository { id } => self.remove_repository(&id),
            SkillsCommand::Discover => self.discover(),
            SkillsCommand::Install { candidate_id, apps } => self.install(&candidate_id, apps, now),
            SkillsCommand::ImportLocal { path, apps } => self.import_local(&path, apps, now),
            SkillsCommand::ScanLocal => self.scan_local(),
            SkillsCommand::SetEnabled {
                skill_id,
                app,
                enabled,
            } => self.set_enabled(&skill_id, app, enabled),
            SkillsCommand::Uninstall { skill_id } => self.uninstall(&skill_id),
            SkillsCommand::Update { skill_id } => self.update(skill_id.as_deref(), now),
            SkillsCommand::SetDailyUpdate { enabled } => {
                let mut next = self.state.clone();
                next.daily_update = enabled;
                next.next_update_at = enabled.then_some(now);
                self.save(next)?;
                Ok(if enabled {
                    "Daily updates enabled; an overdue check runs while the host is running"
                } else {
                    "Daily updates disabled"
                }
                .into())
            }
        }
    }

    fn finish(&mut self, result: Result<String, String>) -> Result<String, String> {
        let mut next = self.state.clone();
        match &result {
            Ok(message) => {
                next.message = files::limited(message, 1000);
                next.last_error = None;
            }
            Err(error) => {
                next.message = "Skills operation failed".into();
                next.last_error = Some(files::limited(error, 1000));
            }
        }
        // Publish the error even when the disk has become unavailable. Mutations
        // themselves still only advance state after their durable write succeeds.
        if let Err(error) = self.save(next.clone()) {
            self.state.message = next.message;
            self.state.last_error = Some(format!(
                "{}; cannot save status: {error}",
                next.last_error.unwrap_or_default()
            ));
            return Err(self.state.last_error.clone().unwrap_or(error));
        }
        result
    }

    fn save(&mut self, next: State) -> Result<(), String> {
        validate_state(&next)?;
        let bytes = state_bytes(&next)?;
        transaction::atomic_write(&self.root.join("state.json"), &bytes)?;
        self.state = next;
        Ok(())
    }

    fn commit(&mut self, mut next: State, changes: Vec<Change>) -> Result<(), String> {
        self.check_cancel()?;
        next.generation = self
            .state
            .generation
            .checked_add(1)
            .ok_or("Skills generation overflow")?;
        validate_state(&next)?;
        let bytes = state_bytes(&next)?;
        let warning = transaction::execute(
            &self.root,
            &self.allowed_parents(),
            next.generation,
            &bytes,
            changes,
        )?;
        self.state = next;
        if let Some(warning) = warning {
            self.state
                .warnings
                .insert(0, files::limited(&warning, 1000));
            self.state.warnings.truncate(16);
        }
        Ok(())
    }

    fn app_root(&self, app: SkillApp) -> PathBuf {
        let suffix = match app {
            SkillApp::Claude => ".claude/skills",
            SkillApp::Codex => ".codex/skills",
            SkillApp::Gemini => ".gemini/skills",
            SkillApp::OpenCode => ".config/opencode/skills",
            SkillApp::Grok => ".grok/skills",
        };
        self.home.join(suffix)
    }

    fn allowed_parents(&self) -> Vec<PathBuf> {
        let mut roots = vec![self.root.join("managed")];
        roots.extend(SkillApp::ALL.into_iter().map(|app| self.app_root(app)));
        roots
    }

    fn check_roots(&self, _create: bool) -> Result<(), String> {
        files::checked_path(&self.root, &self.root.join("managed"), false)?;
        files::checked_path(&self.root, &self.root.join("temporary"), false)?;
        Ok(())
    }

    fn target(&self, app: SkillApp, directory: &str) -> Result<PathBuf, String> {
        let root = self.app_root(app);
        files::checked_path(&self.home, &root, true)?;
        files::name(directory)?;
        Ok(root.join(directory))
    }

    fn check_cancel(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Acquire) {
            Err("Skills operation was cancelled".into())
        } else {
            Ok(())
        }
    }
}

fn state_bytes(state: &State) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(state).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    if bytes.len() > 1024 * 1024 {
        return Err("Skills state exceeds 1 MiB".into());
    }
    Ok(bytes)
}

fn validate_state(state: &State) -> Result<(), String> {
    if state.schema != 1
        || state.repositories.len() > MAX_REPOSITORIES
        || state.installed.len() > MAX_SKILLS
    {
        return Err("Unsupported or oversized skills state".into());
    }
    let mut ids = HashSet::new();
    for repo in &state.repositories {
        files::name(&repo.id)?;
        if !ids.insert(&repo.id)
            || git::repository_url(&repo.url)? != repo.url
            || git::branch(repo.branch.clone())? != repo.branch
        {
            return Err("Invalid or duplicate skills repository".into());
        }
    }
    let mut names = HashSet::new();
    for entry in &state.installed {
        let skill = &entry.skill;
        files::name(&skill.directory)?;
        if skill.id != skill.directory
            || !names.insert(skill.directory.to_lowercase())
            || skill.directory.starts_with(".sean-toolbox-")
        {
            return Err("Invalid or duplicate installed skill directory".into());
        }
        if entry.fingerprint.len() != 64
            || !entry.fingerprint.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err("Invalid installed skill fingerprint".into());
        }
        if skill.apps.iter().collect::<HashSet<_>>().len() != skill.apps.len() {
            return Err("Duplicate skill app".into());
        }
        if let Some(repo) = &skill.repository_id {
            if !ids.contains(repo) {
                return Err("Installed skill references an unknown repository".into());
            }
            files::relative(
                skill
                    .source_path
                    .as_deref()
                    .ok_or("Missing source skill path")?,
            )?;
        } else if skill.source_path.is_some() {
            return Err("Local skill cannot have a repository path".into());
        }
    }
    Ok(())
}

/// Remove staging directories abandoned by an interrupted process. The manager
/// lock is held, so no live checkout or copy can own them. Unknown entries are
/// left untouched, and removal failures become warnings instead of blocking
/// startup.
fn clear_stale_staging(root: &Path) -> Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    for entry in files::sorted_entries(&root.join("temporary"))? {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !(name.starts_with("checkout-") || name.starts_with("skill-")) {
            continue;
        }
        let path = entry.path();
        if !files::metadata(&path).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(&path) {
            warnings.push(files::limited(
                &format!(
                    "Cannot remove stale staging directory {}: {error}",
                    path.display()
                ),
                500,
            ));
        }
    }
    Ok(warnings)
}

impl SkillsService {
    fn add_repository(&mut self, url: String, branch: Option<String>) -> Result<String, String> {
        let url = git::repository_url(&url)?;
        let branch = git::branch(branch)?;
        if self.state.repositories.len() >= MAX_REPOSITORIES {
            return Err("At most 16 repositories can be configured".into());
        }
        if self
            .state
            .repositories
            .iter()
            .any(|repo| repo.url.eq_ignore_ascii_case(&url) && repo.branch == branch)
        {
            return Err("This repository and branch are already configured".into());
        }
        let mut next = self.state.clone();
        let id = format!("repo-{}", next.next_id);
        next.next_id = next
            .next_id
            .checked_add(1)
            .ok_or("Repository id overflow")?;
        next.repositories.push(SkillRepository {
            id,
            url,
            branch,
            revision: None,
            last_error: None,
        });
        self.save(next)?;
        Ok("Repository added; refresh Discover to browse its skills".into())
    }

    fn remove_repository(&mut self, id: &str) -> Result<String, String> {
        if self
            .state
            .installed
            .iter()
            .any(|entry| entry.skill.repository_id.as_deref() == Some(id))
        {
            return Err("This source is used by installed skills; uninstall those skills before removing it".into());
        }
        let mut next = self.state.clone();
        let length = next.repositories.len();
        next.repositories.retain(|repo| repo.id != id);
        if length == next.repositories.len() {
            return Err("Unknown repository".into());
        }
        self.save(next)?;
        self.caches.remove(id);
        self.available
            .retain(|candidate| candidate.repository_id != id);
        Ok("Repository removed".into())
    }

    fn refresh_repository(&mut self, id: &str) -> Result<(), String> {
        // Catalog rows belong to one immutable checkout. Updating that source
        // invalidates its old discovery rows until the next explicit scan.
        self.available
            .retain(|candidate| candidate.repository_id != id);
        let index = self
            .state
            .repositories
            .iter()
            .position(|repo| repo.id == id)
            .ok_or("Unknown repository")?;
        let repository = self.state.repositories[index].clone();
        let result = self
            .fetcher
            .fetch(&repository, &self.root.join("temporary"), &self.cancel);
        match result {
            Ok(checkout) => {
                self.state.repositories[index].revision = Some(checkout.revision.clone());
                self.state.repositories[index].last_error = None;
                self.caches.insert(id.into(), checkout);
                Ok(())
            }
            Err(error) => {
                self.state.repositories[index].last_error = Some(files::limited(&error, 500));
                self.caches.remove(id);
                self.available
                    .retain(|candidate| candidate.repository_id != id);
                Err(error)
            }
        }
    }

    fn discover(&mut self) -> Result<String, String> {
        let repositories = self.state.repositories.clone();
        self.available.clear();
        let mut failures = Vec::new();
        let mut skipped = 0usize;
        for repository in repositories {
            self.check_cancel()?;
            if let Err(error) = self.refresh_repository(&repository.id) {
                failures.push(format!("{}: {error}", repository.url));
                continue;
            }
            let checkout = self
                .caches
                .get(&repository.id)
                .ok_or("Missing repository snapshot")?;
            match candidates(
                &repository,
                checkout,
                MAX_SKILLS.saturating_sub(self.available.len()),
            ) {
                Ok(discovered) => {
                    self.available.extend(discovered.candidates);
                    if discovered.omitted {
                        failures.push(
                            "The catalog exceeds 250 skills; remove a source to view others".into(),
                        );
                    }
                    if !discovered.skipped.is_empty() {
                        skipped += discovered.skipped.len();
                        let sample: Vec<_> = discovered.skipped.iter().take(3).cloned().collect();
                        let more = if discovered.skipped.len() > sample.len() {
                            ", …"
                        } else {
                            ""
                        };
                        self.state.warnings.insert(
                            0,
                            files::limited(
                                &format!(
                                    "{}: skipped {} skill(s) with unreadable SKILL.md: {}{more}",
                                    repository.url,
                                    discovered.skipped.len(),
                                    sample.join(", ")
                                ),
                                500,
                            ),
                        );
                        self.state.warnings.truncate(16);
                    }
                }
                Err(error) => {
                    if let Some(repo) = self
                        .state
                        .repositories
                        .iter_mut()
                        .find(|repo| repo.id == repository.id)
                    {
                        repo.last_error = Some(files::limited(&error, 500));
                    }
                    failures.push(format!("{}: {error}", repository.url));
                }
            }
        }
        self.available.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        if failures.is_empty() {
            let mut message = format!("Discovered {} skills", self.available.len());
            if skipped > 0 {
                message.push_str(&format!("; skipped {skipped} with unreadable SKILL.md"));
            }
            Ok(message)
        } else {
            Err(format!(
                "Found {} skills; {}",
                self.available.len(),
                failures.join("; ")
            ))
        }
    }

    fn source_path(&self, repository: &str, path: &str) -> Result<(PathBuf, String), String> {
        let checkout = self
            .caches
            .get(repository)
            .ok_or("Refresh Discover to load this repository snapshot")?;
        let relative = files::relative(path)?;
        let prefix = format!("{path}/");
        if checkout
            .blocked
            .iter()
            .any(|blocked| path.is_empty() || blocked == path || blocked.starts_with(&prefix))
        {
            return Err("This skill contains unsupported links, paths, or oversized files; no copy was installed".into());
        }
        let source = checkout.tree.join(relative);
        files::checked_path(&checkout.tree, &source, false)?;
        Ok((source, checkout.revision.clone()))
    }

    fn install(
        &mut self,
        candidate_id: &str,
        apps: Vec<SkillApp>,
        now: u64,
    ) -> Result<String, String> {
        let candidate = self
            .available
            .iter()
            .find(|candidate| candidate.id == candidate_id)
            .cloned()
            .ok_or("Skill is no longer in the catalog; refresh Discover")?;
        if self.state.installed.iter().any(|entry| {
            entry.skill.repository_id.as_deref() == Some(&candidate.repository_id)
                && entry.skill.source_path.as_deref() == Some(&candidate.path)
        }) {
            return Err("This skill is already installed; use its app switches or Update".into());
        }
        let (source, revision) = self.source_path(&candidate.repository_id, &candidate.path)?;
        let skill = InstalledSkill {
            id: candidate.directory.clone(),
            directory: candidate.directory,
            name: candidate.name,
            description: candidate.description,
            repository_id: Some(candidate.repository_id),
            source_path: Some(candidate.path),
            revision: Some(revision),
            apps,
            updated_at: now,
            last_error: None,
        };
        self.install_source(&source, skill)?;
        Ok("Skill installed; app copies are managed by Sean Toolbox".into())
    }

    fn import_local(
        &mut self,
        path: &str,
        apps: Vec<SkillApp>,
        now: u64,
    ) -> Result<String, String> {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("Choose an absolute local skill directory".into());
        }
        if !files::metadata(&path)?.is_dir() {
            return Err("Choose a folder containing SKILL.md".into());
        }
        let source = path.canonicalize().map_err(|e| e.to_string())?;
        if source.starts_with(&self.root) {
            return Err("This folder already belongs to the skills manager".into());
        }
        let directory = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid local skill directory")?
            .to_owned();
        files::name(&directory)?;
        let (name, description) = files::descriptor(&source, &directory)?;
        let skill = InstalledSkill {
            id: directory.clone(),
            directory,
            name,
            description,
            repository_id: None,
            source_path: None,
            revision: None,
            apps,
            updated_at: now,
            last_error: None,
        };
        self.install_source(&source, skill)?;
        self.local.retain(|skill| Path::new(&skill.path) != source);
        Ok("Imported a local master copy; local imports have no remote update source".into())
    }

    fn stage_source(&self, source: &Path) -> Result<(tempfile::TempDir, String), String> {
        self.check_cancel()?;
        files::descriptor(source, "skill")?;
        let stage = tempfile::Builder::new()
            .prefix("skill-")
            .tempdir_in(self.root.join("temporary"))
            .map_err(|e| e.to_string())?;
        files::copy_tree(source, stage.path())?;
        let fingerprint = files::fingerprint(stage.path())?;
        Ok((stage, fingerprint))
    }

    fn install_source(&mut self, source: &Path, mut skill: InstalledSkill) -> Result<(), String> {
        if self.state.installed.len() >= MAX_SKILLS {
            return Err("At most 250 skills can be managed".into());
        }
        files::name(&skill.directory)?;
        if skill.directory.starts_with('.') {
            return Err("Skill directory names cannot start with a dot".into());
        }
        if self
            .state
            .installed
            .iter()
            .any(|entry| entry.skill.directory.to_lowercase() == skill.directory.to_lowercase())
        {
            return Err("A skill with this directory name is already installed; names must be unique across sources".into());
        }
        skill.apps.sort();
        skill.apps.dedup();
        let (stage, fingerprint) = self.stage_source(source)?;
        let mut changes = vec![Change {
            destination: self.root.join("managed").join(&skill.directory),
            source: Some(stage.path().to_owned()),
            expected: None,
            replacement: Some(fingerprint.clone()),
        }];
        for app in &skill.apps {
            changes.push(Change {
                destination: self.target(*app, &skill.directory)?,
                source: Some(stage.path().to_owned()),
                expected: None,
                replacement: Some(fingerprint.clone()),
            });
        }
        let mut next = self.state.clone();
        next.installed.push(ManagedSkill { skill, fingerprint });
        self.commit(next, changes)
    }

    fn installed_index(&self, id: &str) -> Result<usize, String> {
        self.state
            .installed
            .iter()
            .position(|entry| entry.skill.id == id)
            .ok_or_else(|| "Unknown installed skill".into())
    }

    fn verify_owned(&self, entry: &ManagedSkill) -> Result<(), String> {
        let central = self.root.join("managed").join(&entry.skill.directory);
        let mut destinations = vec![central];
        for app in &entry.skill.apps {
            let root = self.app_root(*app);
            files::checked_path(&self.home, &root, false)?;
            destinations.push(root.join(&entry.skill.directory));
        }
        for destination in destinations {
            if files::exists(&destination)?
                && files::fingerprint(&destination)? != entry.fingerprint
            {
                return Err(format!(
                    "Local edits were preserved; move or back up the edited folder before retrying: {}",
                    destination.display()
                ));
            }
        }
        Ok(())
    }

    fn set_enabled(&mut self, id: &str, app: SkillApp, enabled: bool) -> Result<String, String> {
        let index = self.installed_index(id)?;
        let entry = self.state.installed[index].clone();
        self.verify_owned(&entry)?;
        if !enabled && !entry.skill.apps.contains(&app) {
            return Ok("Skill is already disabled for this app".into());
        }
        let source = self.root.join("managed").join(&entry.skill.directory);
        if enabled && !files::exists(&source)? {
            return Err(
                "The managed master copy is missing; update or reimport the skill first".into(),
            );
        }
        let target = self.target(app, &entry.skill.directory)?;
        let mut next = self.state.clone();
        next.installed[index]
            .skill
            .apps
            .retain(|current| *current != app);
        if enabled {
            next.installed[index].skill.apps.push(app);
            next.installed[index].skill.apps.sort();
        }
        next.installed[index].skill.last_error = None;
        let expected = entry
            .skill
            .apps
            .contains(&app)
            .then_some(entry.fingerprint.clone());
        self.commit(
            next,
            vec![Change {
                destination: target,
                source: enabled.then_some(source),
                expected,
                replacement: enabled.then_some(entry.fingerprint),
            }],
        )?;
        Ok(format!(
            "{} copy {}",
            app.label(),
            if enabled { "enabled" } else { "removed" }
        ))
    }

    fn uninstall(&mut self, id: &str) -> Result<String, String> {
        let index = self.installed_index(id)?;
        let entry = self.state.installed[index].clone();
        self.verify_owned(&entry)?;
        let mut changes = vec![Change {
            destination: self.root.join("managed").join(&entry.skill.directory),
            source: None,
            expected: Some(entry.fingerprint.clone()),
            replacement: None,
        }];
        for app in &entry.skill.apps {
            changes.push(Change {
                destination: self.target(*app, &entry.skill.directory)?,
                source: None,
                expected: Some(entry.fingerprint.clone()),
                replacement: None,
            });
        }
        let mut next = self.state.clone();
        next.installed.remove(index);
        self.commit(next, changes)?;
        Ok("Skill uninstalled; unrelated folders were left untouched".into())
    }

    fn scan_local(&mut self) -> Result<String, String> {
        self.local.clear();
        let mut roots: Vec<_> = SkillApp::ALL
            .into_iter()
            .map(|app| (Some(app), self.app_root(app)))
            .collect();
        roots.push((None, self.home.join(".cc-switch/skills")));
        roots.push((None, self.home.join(".agents/skills")));
        let mut errors = Vec::new();
        let mut visited = 0;
        for (app, root) in roots {
            if let Err(error) = files::checked_path(&self.home, &root, false) {
                errors.push(error);
                continue;
            }
            if !files::exists(&root)? {
                continue;
            }
            for entry in files::sorted_entries(&root)? {
                self.check_cancel()?;
                visited += 1;
                if visited > 4096 || errors.len() >= 16 {
                    errors.push("Local scan stopped at its entry/error budget".into());
                    break;
                }
                let directory = entry.file_name();
                let Some(directory) = directory.to_str() else {
                    continue;
                };
                if directory.starts_with('.') || files::name(directory).is_err() {
                    continue;
                }
                let path = entry.path();
                if !files::metadata(&path).is_ok_and(|meta| meta.is_dir())
                    || !path.join("SKILL.md").exists()
                {
                    continue;
                }
                if app.is_some_and(|app| {
                    self.state.installed.iter().any(|entry| {
                        entry.skill.directory == directory && entry.skill.apps.contains(&app)
                    })
                }) {
                    continue;
                }
                match files::descriptor(&path, directory) {
                    Ok((name, description)) => {
                        if self.local.len() >= MAX_SKILLS {
                            errors.push("Local scan exceeds 250 skills".into());
                            break;
                        }
                        self.local.push(LocalSkill {
                            path: path.to_string_lossy().into_owned(),
                            directory: directory.into(),
                            name,
                            description,
                            app,
                        });
                    }
                    Err(error) => errors.push(error),
                }
            }
        }
        if errors.is_empty() {
            Ok(format!("Found {} unmanaged local skills", self.local.len()))
        } else {
            Err(format!(
                "Found {} local skills; {}",
                self.local.len(),
                errors.join("; ")
            ))
        }
    }

    fn update(&mut self, id: Option<&str>, now: u64) -> Result<String, String> {
        let entries: Vec<_> = if let Some(id) = id {
            let index = self.installed_index(id)?;
            if self.state.installed[index].skill.repository_id.is_none() {
                return Err("Local imports have no repository; reimport them to update".into());
            }
            vec![self.state.installed[index].clone()]
        } else {
            let mut next = self.state.clone();
            next.last_attempt_at = Some(now);
            next.next_update_at = Some(now.saturating_add(RETRY));
            self.save(next)?;
            self.state
                .installed
                .iter()
                .filter(|entry| entry.skill.repository_id.is_some())
                .cloned()
                .collect()
        };
        let mut sources = BTreeMap::<String, Result<(), String>>::new();
        let mut changed = 0;
        let mut failures = Vec::new();
        for entry in entries {
            self.check_cancel()?;
            let source = entry
                .skill
                .repository_id
                .as_ref()
                .ok_or("Missing repository")?;
            if !sources.contains_key(source) {
                sources.insert(source.clone(), self.refresh_repository(source));
            }
            let result = match sources.get(source).ok_or("Missing update result")? {
                Ok(()) => self.update_skill(&entry.skill.id, now),
                Err(error) => Err(error.clone()),
            };
            let index = self.installed_index(&entry.skill.id)?;
            match result {
                Ok(updated) => {
                    changed += usize::from(updated);
                    self.state.installed[index].skill.last_error = None;
                }
                Err(error) => {
                    self.state.installed[index].skill.last_error =
                        Some(files::limited(&error, 500));
                    failures.push(format!("{}: {error}", entry.skill.name));
                }
            }
            if files::exists(&self.root.join("pending.json"))? {
                failures.push(
                    "The remaining skills were not updated because a transaction needs recovery"
                        .into(),
                );
                break;
            }
        }
        if id.is_none() {
            let mut next = self.state.clone();
            next.next_update_at =
                Some(now.saturating_add(if failures.is_empty() { DAY } else { RETRY }));
            if failures.is_empty() {
                next.last_success_at = Some(now);
            }
            self.save(next)?;
        }
        if failures.is_empty() {
            Ok(format!("Skills update complete; {changed} changed"))
        } else {
            Err(format!(
                "Updated {changed} skills; {} failed: {}",
                failures.len(),
                failures.join("; ")
            ))
        }
    }

    fn update_skill(&mut self, id: &str, now: u64) -> Result<bool, String> {
        let index = self.installed_index(id)?;
        let entry = self.state.installed[index].clone();
        self.verify_owned(&entry)?;
        let repository = entry
            .skill
            .repository_id
            .as_deref()
            .ok_or("Local imports do not have updates")?;
        let path = entry
            .skill
            .source_path
            .as_deref()
            .ok_or("Missing skill source path")?;
        let (source, revision) = self.source_path(repository, path)?;
        let (stage, fingerprint) = self.stage_source(&source)?;
        let (name, description) = files::descriptor(stage.path(), &entry.skill.directory)?;
        let mut changes = Vec::new();
        let mut targets = vec![self.root.join("managed").join(&entry.skill.directory)];
        for app in &entry.skill.apps {
            targets.push(self.target(*app, &entry.skill.directory)?);
        }
        for destination in targets {
            if fingerprint != entry.fingerprint || !files::exists(&destination)? {
                changes.push(Change {
                    destination,
                    source: Some(stage.path().to_owned()),
                    expected: Some(entry.fingerprint.clone()),
                    replacement: Some(fingerprint.clone()),
                });
            }
        }
        let changed = !changes.is_empty();
        let mut next = self.state.clone();
        let updated = &mut next.installed[index];
        updated.fingerprint = fingerprint;
        updated.skill.name = name;
        updated.skill.description = description;
        updated.skill.revision = Some(revision);
        updated.skill.last_error = None;
        if changed {
            updated.skill.updated_at = now;
        }
        self.commit(next, changes)?;
        Ok(changed)
    }
}

struct Discovered {
    candidates: Vec<SkillCandidate>,
    omitted: bool,
    /// Packages whose SKILL.md could not be read. They are reported without
    /// hiding the rest of the repository catalog.
    skipped: Vec<String>,
}

fn candidates(
    repository: &SkillRepository,
    checkout: &git::Checkout,
    limit: usize,
) -> Result<Discovered, String> {
    let mut pending = vec![(checkout.tree.clone(), String::new(), 0)];
    let mut found = Vec::new();
    let mut visited = 0;
    let mut omitted = false;
    let mut skipped = Vec::new();
    while let Some((directory, relative, depth)) = pending.pop() {
        visited += 1;
        if visited > 20_000 || depth > 20 {
            return Err("Repository discovery exceeds its directory budget".into());
        }
        if directory.join("SKILL.md").is_file() {
            let leaf = if relative.is_empty() {
                repository
                    .url
                    .trim_end_matches(".git")
                    .rsplit('/')
                    .next()
                    .unwrap_or("skill")
            } else {
                relative.rsplit('/').next().unwrap_or("skill")
            };
            if files::name(leaf).is_ok() && !leaf.starts_with('.') {
                match files::descriptor(&directory, leaf) {
                    Ok((name, description)) => {
                        if found.len() < limit {
                            found.push(SkillCandidate {
                                id: format!("{}:{relative}", repository.id),
                                repository_id: repository.id.clone(),
                                path: relative.clone(),
                                directory: leaf.into(),
                                name,
                                description,
                                installed: false,
                            });
                        } else {
                            omitted = true;
                        }
                    }
                    Err(_) => skipped.push(if relative.is_empty() {
                        leaf.into()
                    } else {
                        relative.clone()
                    }),
                }
            }
            // A SKILL.md defines a package boundary. Nested examples may carry
            // SKILL.md fixtures and stay part of the containing package.
            continue;
        }
        for entry in files::sorted_entries(&directory)?.into_iter().rev() {
            let file_name = entry.file_name();
            let Some(name) = file_name.to_str() else {
                continue;
            };
            if matches!(
                name,
                ".git" | ".github" | ".worktrees" | "node_modules" | "target" | "__pycache__"
            ) {
                continue;
            }
            if files::metadata(&entry.path()).is_ok_and(|meta| meta.is_dir()) {
                let path = if relative.is_empty() {
                    name.into()
                } else {
                    format!("{relative}/{name}")
                };
                if files::relative(&path).is_ok() {
                    pending.push((entry.path(), path, depth + 1));
                }
            }
        }
    }
    Ok(Discovered {
        candidates: found,
        omitted,
        skipped,
    })
}

#[cfg(test)]
mod tests;
