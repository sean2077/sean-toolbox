use super::*;
use std::{ffi::OsString, process::Command, sync::atomic::AtomicUsize};

struct LocalFetcher {
    origin: PathBuf,
    fail: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}

impl git::Fetcher for LocalFetcher {
    fn fetch(
        &self,
        _repository: &SkillRepository,
        root: &Path,
        cancel: &AtomicBool,
    ) -> Result<git::Checkout, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::Acquire) {
            return Err("Fixture network is offline".into());
        }
        let temporary = tempfile::tempdir_in(root).map_err(|e| e.to_string())?;
        let repository = self.origin.join(".git");
        let revision = git::run(
            temporary.path(),
            [
                OsString::from("--git-dir"),
                repository.clone().into_os_string(),
                OsString::from("rev-parse"),
                OsString::from("HEAD"),
            ],
            cancel,
            None,
        )?
        .trim()
        .to_owned();
        git::archive_repository(temporary, &repository, revision, cancel)
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    origin: PathBuf,
    fail: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
    service: Option<SkillsService>,
}

impl Fixture {
    fn new() -> Self {
        // SkillsService canonicalizes this root (a verbatim path on Windows).
        // Keep spaces in it so the real Git/archive fixtures cover both cases.
        let temp = tempfile::Builder::new()
            .prefix("skills fixture ")
            .tempdir()
            .unwrap();
        let root = temp.path().join("configuration/skills");
        let home = temp.path().join("home");
        let origin = temp.path().join("origin");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&origin).unwrap();
        git_command(&origin, &["init", "--initial-branch=main"]);
        let fail = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut fixture = Self {
            _temp: temp,
            root,
            home,
            origin,
            fail,
            calls,
            service: None,
        };
        fixture.reopen();
        fixture
            .service()
            .execute(
                SkillsCommand::RemoveRepository {
                    id: "repo-1".into(),
                },
                0,
            )
            .unwrap();
        fixture
            .service()
            .execute(
                SkillsCommand::AddRepository {
                    url: "fixture/skills".into(),
                    branch: Some("main".into()),
                },
                0,
            )
            .unwrap();
        fixture
    }

    fn service(&mut self) -> &mut SkillsService {
        self.service.as_mut().unwrap()
    }

    fn reopen(&mut self) {
        self.service.take();
        let mut service = SkillsService::open(
            self.root.clone(),
            self.home.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        service.fetcher = Box::new(LocalFetcher {
            origin: self.origin.clone(),
            fail: self.fail.clone(),
            calls: self.calls.clone(),
        });
        self.service = Some(service);
    }

    fn commit(&self, name: &str, content: &str) {
        let skill = self.origin.join("skills").join(name);
        fs::create_dir_all(&skill).unwrap();
        fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Fixture {content}\n---\n{content}\n"),
        )
        .unwrap();
        fs::write(skill.join("example.txt"), content).unwrap();
        git_command(&self.origin, &["add", "."]);
        git_command(
            &self.origin,
            &[
                "-c",
                "user.name=Skills Test",
                "-c",
                "user.email=skills-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                content,
            ],
        );
    }

    fn install(&mut self, directory: &str, apps: Vec<SkillApp>, now: u64) {
        self.service()
            .execute(SkillsCommand::Discover, now)
            .unwrap();
        let id = self
            .service()
            .snapshot()
            .available
            .into_iter()
            .find(|skill| skill.directory == directory)
            .unwrap()
            .id;
        self.service()
            .execute(
                SkillsCommand::Install {
                    candidate_id: id,
                    apps,
                },
                now,
            )
            .unwrap();
    }

    fn master(&self, skill: &str) -> PathBuf {
        self.root.join("managed").join(skill)
    }
    fn app(&self, app: SkillApp, skill: &str) -> PathBuf {
        self.service.as_ref().unwrap().app_root(app).join(skill)
    }
}

fn git_command(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .expect("Git is required for offline skills fixtures");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn file_content(path: &Path) -> String {
    fs::read_to_string(path.join("example.txt")).unwrap()
}

#[test]
fn repository_install_switch_update_and_restart_preserve_disabled_apps() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old revision");
    fixture.install("review", vec![SkillApp::Claude, SkillApp::Codex], 10);
    assert_eq!(file_content(&fixture.master("review")), "old revision");
    for app in [SkillApp::Claude, SkillApp::Codex] {
        assert_eq!(file_content(&fixture.app(app, "review")), "old revision");
    }
    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabled {
                skill_id: "review".into(),
                app: SkillApp::Claude,
                enabled: false,
            },
            11,
        )
        .unwrap();
    fixture.commit("review", "new revision");
    fixture
        .service()
        .execute(SkillsCommand::Update { skill_id: None }, 20)
        .unwrap();
    assert_eq!(file_content(&fixture.master("review")), "new revision");
    assert_eq!(
        file_content(&fixture.app(SkillApp::Codex, "review")),
        "new revision"
    );
    assert!(!fixture.app(SkillApp::Claude, "review").exists());
    fixture.reopen();
    let snapshot = fixture.service().snapshot();
    assert_eq!(snapshot.installed[0].apps, vec![SkillApp::Codex]);
    assert_eq!(snapshot.last_success_at, Some(20));
    assert!(!fixture.root.join("pending.json").exists());
    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "review".into(),
            },
            30,
        )
        .unwrap();
    assert!(!fixture.master("review").exists());
    assert!(!fixture.app(SkillApp::Codex, "review").exists());
}

#[test]
fn local_edit_rejects_update_disable_and_uninstall_without_partial_changes() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    fixture.install("review", vec![SkillApp::Claude, SkillApp::Codex], 0);
    fs::write(
        fixture.app(SkillApp::Codex, "review").join("example.txt"),
        "user edits",
    )
    .unwrap();
    fixture.commit("review", "remote change");
    for command in [
        SkillsCommand::Update { skill_id: None },
        SkillsCommand::SetEnabled {
            skill_id: "review".into(),
            app: SkillApp::Codex,
            enabled: false,
        },
        SkillsCommand::Uninstall {
            skill_id: "review".into(),
        },
    ] {
        assert!(fixture.service().execute(command, 20).is_err());
    }
    assert_eq!(
        file_content(&fixture.app(SkillApp::Codex, "review")),
        "user edits"
    );
    assert_eq!(
        file_content(&fixture.app(SkillApp::Claude, "review")),
        "old"
    );
    assert_eq!(file_content(&fixture.master("review")), "old");
    assert!(
        fixture.service().snapshot().installed[0]
            .last_error
            .is_some()
    );
}

#[test]
fn identical_unmanaged_folders_are_never_adopted_or_deleted() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "original");
    let existing = fixture.app(SkillApp::Codex, "review");
    fs::create_dir_all(&existing).unwrap();
    files::copy_tree(&fixture.origin.join("skills/review"), &existing).unwrap();
    fixture
        .service()
        .execute(SkillsCommand::Discover, 0)
        .unwrap();
    let id = fixture.service().snapshot().available[0].id.clone();
    let result = fixture.service().execute(
        SkillsCommand::Install {
            candidate_id: id.clone(),
            apps: vec![SkillApp::Claude, SkillApp::Codex],
        },
        0,
    );
    assert!(result.is_err());
    assert!(!fixture.master("review").exists());
    assert!(!fixture.app(SkillApp::Claude, "review").exists());
    assert_eq!(file_content(&existing), "original");
    fixture
        .service()
        .execute(
            SkillsCommand::Install {
                candidate_id: id,
                apps: vec![],
            },
            0,
        )
        .unwrap();
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::SetEnabled {
                    skill_id: "review".into(),
                    app: SkillApp::Codex,
                    enabled: true
                },
                0
            )
            .is_err()
    );
    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "review".into(),
            },
            0,
        )
        .unwrap();
    assert_eq!(file_content(&existing), "original");
}

#[test]
fn local_import_scans_cc_switch_and_preserves_original_on_uninstall() {
    let mut fixture = Fixture::new();
    let original = fixture.home.join(".cc-switch/skills/local-review");
    fs::create_dir_all(&original).unwrap();
    fs::write(
        original.join("SKILL.md"),
        "---\nname: Local Review\ndescription: |\n  First line\n  second line\n---\nInstructions",
    )
    .unwrap();
    fs::write(original.join("example.txt"), "local original").unwrap();
    fs::create_dir(original.join(".git")).unwrap();
    fs::write(
        original.join(".git/private"),
        "never copy repository metadata",
    )
    .unwrap();
    fixture
        .service()
        .execute(SkillsCommand::ScanLocal, 0)
        .unwrap();
    assert_eq!(
        fixture.service().snapshot().local[0].description,
        "First line second line"
    );
    fixture
        .service()
        .execute(
            SkillsCommand::ImportLocal {
                path: original.to_string_lossy().into_owned(),
                apps: vec![SkillApp::Gemini],
            },
            1,
        )
        .unwrap();
    assert!(!fixture.master("local-review").join(".git").exists());
    assert_eq!(
        file_content(&fixture.app(SkillApp::Gemini, "local-review")),
        "local original"
    );
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::Update {
                    skill_id: Some("local-review".into())
                },
                2
            )
            .is_err()
    );
    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "local-review".into(),
            },
            3,
        )
        .unwrap();
    assert_eq!(file_content(&original), "local original");
    assert_eq!(
        fs::read_to_string(original.join(".git/private")).unwrap(),
        "never copy repository metadata"
    );
}

#[test]
fn daily_update_persists_catchup_and_failure_retry_without_repeated_fetches() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    fixture.install("review", vec![SkillApp::Claude], 1);
    assert_eq!(fixture.service().next_due(), None);
    fixture
        .service()
        .execute(SkillsCommand::SetDailyUpdate { enabled: true }, 100)
        .unwrap();
    assert_eq!(fixture.service().next_due(), Some(100));
    fixture.service().run_daily(100).unwrap();
    assert_eq!(fixture.service().next_due(), Some(100 + DAY));
    let calls = fixture.calls.load(Ordering::SeqCst);
    fixture.service().run_daily(101).unwrap();
    assert_eq!(fixture.calls.load(Ordering::SeqCst), calls);
    fixture.reopen();
    fixture.fail.store(true, Ordering::Release);
    assert!(fixture.service().run_daily(200 + DAY).is_err());
    let snapshot = fixture.service().snapshot();
    assert_eq!(snapshot.next_update_at, Some(200 + DAY + RETRY));
    assert_eq!(snapshot.last_success_at, Some(100));
    assert!(
        snapshot.repositories[0]
            .last_error
            .as_deref()
            .unwrap()
            .contains("offline")
    );
    assert_eq!(file_content(&fixture.master("review")), "old");
    fixture.reopen();
    assert_eq!(fixture.service().next_due(), Some(200 + DAY + RETRY));
    fixture.fail.store(false, Ordering::Release);
    fixture.commit("review", "reconnected");
    fixture.service().run_daily(200 + DAY + RETRY).unwrap();
    assert_eq!(
        file_content(&fixture.app(SkillApp::Claude, "review")),
        "reconnected"
    );
    assert!(fixture.service().snapshot().last_error.is_none());
    fixture
        .service()
        .execute(SkillsCommand::SetDailyUpdate { enabled: false }, 1_000_000)
        .unwrap();
    fixture.reopen();
    assert_eq!(fixture.service().next_due(), None);
}

#[test]
fn missing_remote_skill_and_case_collisions_preserve_installed_versions() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    fixture.install("review", vec![], 1);
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::RemoveRepository {
                    id: "repo-2".into()
                },
                1
            )
            .is_err()
    );
    fs::rename(
        fixture.origin.join("skills/review"),
        fixture.origin.join("skills/moved"),
    )
    .unwrap();
    git_command(&fixture.origin, &["add", "."]);
    git_command(
        &fixture.origin,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "move skill",
        ],
    );
    assert!(
        fixture
            .service()
            .execute(SkillsCommand::Update { skill_id: None }, 2)
            .is_err()
    );
    assert_eq!(file_content(&fixture.master("review")), "old");
    let local = fixture.home.join("Review");
    fs::create_dir(&local).unwrap();
    fs::write(local.join("SKILL.md"), "Another skill").unwrap();
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::ImportLocal {
                    path: local.to_string_lossy().into_owned(),
                    apps: vec![]
                },
                3
            )
            .is_err()
    );
    assert_eq!(fixture.service().snapshot().installed.len(), 1);
}

#[test]
fn repository_and_path_validation_rejects_injection_and_windows_aliases() {
    assert_eq!(
        git::repository_url("owner/repo").unwrap(),
        "https://github.com/owner/repo.git"
    );
    assert_eq!(
        git::repository_url("https://github.com/owner/repo.git/").unwrap(),
        "https://github.com/owner/repo.git"
    );
    for value in [
        "file:///tmp/repo",
        "git@github.com:owner/repo",
        "https://github.com.evil/owner/repo",
        "https://token@github.com/owner/repo",
        "https://github.com/owner/repo/tree/main",
        "--upload-pack=evil/repo",
        "owner/../repo",
    ] {
        assert!(git::repository_url(value).is_err(), "{value}");
    }
    for value in [
        "--upload-pack=evil",
        "../main",
        "branch.lock",
        "a//b",
        "main@{1}",
        "a b",
    ] {
        assert!(git::branch(Some(value.into())).is_err(), "{value}");
    }
    assert_eq!(
        git::branch(Some("feature/skills".into())).unwrap(),
        Some("feature/skills".into())
    );
    for value in [
        "../escape",
        "/absolute",
        "C:/escape",
        "parent\\child",
        "CON",
        "con.md",
        "aux",
        "skill.",
        "skill ",
        "a:b",
    ] {
        assert!(files::relative(value).is_err(), "{value}");
    }
}

#[cfg(unix)]
#[test]
fn symlink_sources_and_redirected_targets_fail_without_touching_victims() {
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    let victim = fixture.home.join("victim");
    fs::create_dir(&victim).unwrap();
    fs::write(victim.join("safe"), "do not touch").unwrap();
    symlink(&victim, fixture.origin.join("skills/review/link")).unwrap();
    git_command(&fixture.origin, &["add", "."]);
    git_command(
        &fixture.origin,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "symlink",
        ],
    );
    fixture
        .service()
        .execute(SkillsCommand::Discover, 0)
        .unwrap();
    let id = fixture.service().snapshot().available[0].id.clone();
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::Install {
                    candidate_id: id,
                    apps: vec![]
                },
                1
            )
            .is_err()
    );
    assert!(!fixture.master("review").exists());
    let local = fixture.home.join("local");
    fs::create_dir(&local).unwrap();
    fs::write(local.join("SKILL.md"), "Safe instructions").unwrap();
    symlink(&victim, fixture.home.join(".grok")).unwrap();
    fixture.reopen(); // An unrelated unsafe app path does not disable the manager.
    fixture
        .service()
        .execute(
            SkillsCommand::ImportLocal {
                path: local.to_string_lossy().into_owned(),
                apps: vec![SkillApp::Claude],
            },
            1,
        )
        .unwrap();
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::SetEnabled {
                    skill_id: "local".into(),
                    app: SkillApp::Grok,
                    enabled: true
                },
                1
            )
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(victim.join("safe")).unwrap(),
        "do not touch"
    );
    assert!(!victim.join("skills").exists());
}

#[cfg(unix)]
#[test]
fn executable_scripts_keep_their_mode_across_install_and_update() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    let script = fixture.origin.join("skills/review/run.sh");
    fs::write(
        &script,
        "#!/bin/sh\nprintf 'script is data, never run by the manager'\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    git_command(&fixture.origin, &["add", "."]);
    git_command(
        &fixture.origin,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "script",
        ],
    );
    fixture.install("review", vec![SkillApp::Claude], 1);
    assert_ne!(
        fs::metadata(fixture.app(SkillApp::Claude, "review").join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    fixture.commit("review", "updated");
    fixture
        .service()
        .execute(SkillsCommand::Update { skill_id: None }, 2)
        .unwrap();
    assert_ne!(
        fs::metadata(fixture.master("review").join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
}

#[test]
fn state_lock_corruption_and_cancellation_preserve_user_files() {
    let mut fixture = Fixture::new();
    assert!(
        SkillsService::open(
            fixture.root.clone(),
            fixture.home.clone(),
            Arc::new(AtomicBool::new(false))
        )
        .is_err()
    );
    fixture.service().cancel.store(true, Ordering::Release);
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::AddRepository {
                    url: "other/repo".into(),
                    branch: None
                },
                0
            )
            .is_err()
    );
    assert_eq!(fixture.service().snapshot().repositories.len(), 1);
    fixture.service.take();
    fs::write(fixture.root.join("state.json"), "broken JSON").unwrap();
    assert!(
        SkillsService::open(
            fixture.root.clone(),
            fixture.home.clone(),
            Arc::new(AtomicBool::new(false))
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("state.json")).unwrap(),
        "broken JSON"
    );
}

#[test]
fn snapshot_byte_budget_remains_below_ipc_limit_with_unicode_metadata() {
    let mut fixture = Fixture::new();
    let service = fixture.service();
    for index in 0..MAX_SKILLS {
        let directory = format!("skill-{index}");
        let name = "🦀".repeat(80);
        let description = "🦀".repeat(240);
        let source_path = "x".repeat(512);
        service.state.installed.push(ManagedSkill {
            skill: InstalledSkill {
                id: directory.clone(),
                directory: directory.clone(),
                name: name.clone(),
                description: description.clone(),
                repository_id: Some("repo-2".into()),
                source_path: Some(source_path.clone()),
                revision: Some("a".repeat(64)),
                apps: SkillApp::ALL.to_vec(),
                updated_at: 10,
                last_error: Some("🦀".repeat(500)),
            },
            fingerprint: "0".repeat(64),
        });
        service.available.push(SkillCandidate {
            id: format!("repo-2:{source_path}{index}"),
            repository_id: "repo-2".into(),
            path: source_path,
            directory: directory.clone(),
            name: name.clone(),
            description: description.clone(),
            installed: false,
        });
        service.local.push(LocalSkill {
            path: format!("/{}", "x".repeat(512)),
            directory,
            name,
            description,
            app: Some(SkillApp::Claude),
        });
    }
    let snapshot = service.snapshot();
    assert!(snapshot.truncated);
    assert!(serde_json::to_vec(&snapshot).unwrap().len() <= 700 * 1024);
}

#[test]
fn package_examples_remain_inside_their_skill_boundary() {
    let mut fixture = Fixture::new();
    fixture.commit("authoring", "package");
    let example = fixture.origin.join("skills/authoring/examples/demo");
    fs::create_dir_all(&example).unwrap();
    fs::write(
        example.join("SKILL.md"),
        "Example file, not a separate install",
    )
    .unwrap();
    git_command(&fixture.origin, &["add", "."]);
    git_command(
        &fixture.origin,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "nested fixture",
        ],
    );
    fixture.install("authoring", vec![], 1);
    assert_eq!(fixture.service().snapshot().available.len(), 1);
    assert_eq!(
        fs::read_to_string(fixture.master("authoring").join("examples/demo/SKILL.md")).unwrap(),
        "Example file, not a separate install"
    );
}

#[test]
fn submodule_dependencies_are_rejected_without_fetching_or_installing_them() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "package");
    let revision = git_command(&fixture.origin, &["rev-parse", "HEAD"])
        .trim()
        .to_owned();
    git_command(
        &fixture.origin,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{revision},skills/review/dependency"),
        ],
    );
    git_command(
        &fixture.origin,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "gitlink fixture",
        ],
    );
    fixture
        .service()
        .execute(SkillsCommand::Discover, 1)
        .unwrap();
    let id = fixture.service().snapshot().available[0].id.clone();
    assert!(
        fixture
            .service()
            .execute(
                SkillsCommand::Install {
                    candidate_id: id,
                    apps: vec![]
                },
                1
            )
            .is_err()
    );
    assert!(!fixture.master("review").exists());
}

#[test]
fn a_failed_skill_does_not_prevent_other_safe_updates_or_mark_batch_successful() {
    let mut fixture = Fixture::new();
    fixture.commit("edited", "old one");
    fixture.commit("safe", "old two");
    fixture.install("edited", vec![], 1);
    fixture.install("safe", vec![SkillApp::Grok], 1);
    fixture
        .service()
        .execute(SkillsCommand::SetDailyUpdate { enabled: true }, 10)
        .unwrap();
    fs::write(fixture.master("edited").join("example.txt"), "local edits").unwrap();
    fixture.commit("edited", "remote one");
    fixture.commit("safe", "remote two");
    assert!(fixture.service().run_daily(10).is_err());
    assert_eq!(file_content(&fixture.master("edited")), "local edits");
    assert_eq!(
        file_content(&fixture.app(SkillApp::Grok, "safe")),
        "remote two"
    );
    let snapshot = fixture.service().snapshot();
    assert_eq!(snapshot.next_update_at, Some(10 + RETRY));
    assert_eq!(snapshot.last_success_at, None);
    assert!(
        snapshot
            .installed
            .iter()
            .find(|entry| entry.id == "edited")
            .unwrap()
            .last_error
            .is_some()
    );
    assert!(
        snapshot
            .installed
            .iter()
            .find(|entry| entry.id == "safe")
            .unwrap()
            .last_error
            .is_none()
    );
}
