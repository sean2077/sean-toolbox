use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde_json::Value;
use url::Url;

use crate::{
    DiagnosticLevel, ProjectId, ProjectKind, ProjectTarget, ProviderDiagnostic, ProviderId,
    ProviderReport, RecentProject, RecentProjectProvider, TargetIdentityPolicy,
    deduplicate_with_policy, target_key_with_policy,
};

pub const RECENT_KEY: &str = "history.recentlyOpenedPathsList";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKind {
    Windows,
    Linux,
    MacOs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VsCodePlatformLayout {
    platform: PlatformKind,
    home: PathBuf,
    config_root: PathBuf,
    data_root: PathBuf,
    application_roots: Vec<PathBuf>,
    executable_path: Vec<PathBuf>,
}

pub type DiscoveryEnvironment = VsCodePlatformLayout;

impl VsCodePlatformLayout {
    pub fn current() -> Self {
        let path = env::var_os("PATH")
            .map(|value| env::split_paths(&value).collect())
            .unwrap_or_default();

        #[cfg(target_os = "windows")]
        {
            let value = |name: &str| env::var_os(name).map(PathBuf::from).unwrap_or_default();
            let home = value("USERPROFILE");
            let roaming_data = value("APPDATA");
            let local_data = value("LOCALAPPDATA");
            let mut application_roots = Vec::new();
            for (root, suffix) in [
                (&local_data, "Programs/Microsoft VS Code"),
                (&value("ProgramFiles"), "Microsoft VS Code"),
                (&value("ProgramFiles(x86)"), "Microsoft VS Code"),
            ] {
                if !root.as_os_str().is_empty() {
                    application_roots.push(root.join(suffix));
                }
            }
            return Self::windows(home, roaming_data, local_data, application_roots, path);
        }

        #[cfg(target_os = "linux")]
        {
            let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            let config_root = env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            let data_root = env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share"));
            let application_roots = vec![
                PathBuf::from("/usr/share/code"),
                PathBuf::from("/usr/lib/code"),
                PathBuf::from("/opt/visual-studio-code"),
                data_root.join("code"),
                home.join("snap/code/current/usr/share/code"),
            ];
            return Self::linux(home, config_root, data_root, application_roots, path);
        }

        #[cfg(target_os = "macos")]
        {
            let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            let application_support = home.join("Library/Application Support");
            let application_roots = vec![
                PathBuf::from("/Applications/Visual Studio Code.app"),
                home.join("Applications/Visual Studio Code.app"),
            ];
            return Self::macos(home, application_support, application_roots, path);
        }

        #[allow(unreachable_code)]
        Self::linux(
            PathBuf::new(),
            PathBuf::new(),
            PathBuf::new(),
            Vec::new(),
            path,
        )
    }

    pub fn windows(
        home: PathBuf,
        roaming_data: PathBuf,
        local_data: PathBuf,
        application_roots: Vec<PathBuf>,
        executable_path: Vec<PathBuf>,
    ) -> Self {
        Self {
            platform: PlatformKind::Windows,
            home,
            config_root: roaming_data,
            data_root: local_data,
            application_roots,
            executable_path,
        }
    }

    pub fn linux(
        home: PathBuf,
        config_root: PathBuf,
        data_root: PathBuf,
        application_roots: Vec<PathBuf>,
        executable_path: Vec<PathBuf>,
    ) -> Self {
        Self {
            platform: PlatformKind::Linux,
            home,
            config_root,
            data_root,
            application_roots,
            executable_path,
        }
    }

    pub fn macos(
        home: PathBuf,
        application_support: PathBuf,
        application_roots: Vec<PathBuf>,
        executable_path: Vec<PathBuf>,
    ) -> Self {
        Self {
            platform: PlatformKind::MacOs,
            home,
            config_root: application_support.clone(),
            data_root: application_support,
            application_roots,
            executable_path,
        }
    }

    pub const fn platform(&self) -> PlatformKind {
        self.platform
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn config_root(&self) -> &Path {
        &self.config_root
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub const fn target_identity_policy(&self) -> TargetIdentityPolicy {
        match self.platform {
            PlatformKind::Windows => TargetIdentityPolicy::CaseInsensitiveAscii,
            PlatformKind::Linux | PlatformKind::MacOs => TargetIdentityPolicy::CaseSensitive,
        }
    }

    fn executable_candidates(&self) -> Vec<PathBuf> {
        let mut candidates = Vec::new();
        for directory in &self.executable_path {
            match self.platform {
                PlatformKind::Windows => {
                    candidates.push(directory.join("Code.exe"));
                    candidates.push(directory.join("code.exe"));
                    if directory
                        .file_name()
                        .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
                        && let Some(parent) = directory.parent()
                    {
                        candidates.push(parent.join("Code.exe"));
                    }
                }
                PlatformKind::Linux | PlatformKind::MacOs => {
                    candidates.push(directory.join("code"));
                }
            }
        }
        for root in &self.application_roots {
            match self.platform {
                PlatformKind::Windows => candidates.push(root.join("Code.exe")),
                PlatformKind::Linux => {
                    candidates.push(root.join("code"));
                    candidates.push(root.join("bin/code"));
                }
                PlatformKind::MacOs => {
                    candidates.push(root.join("Contents/Resources/app/bin/code"));
                }
            }
        }
        unique_paths(candidates, self.target_identity_policy())
    }

    fn normalize_executable_candidate(&self, path: &Path) -> PathBuf {
        if path.is_dir() {
            return match self.platform {
                PlatformKind::Windows => path.join("Code.exe"),
                PlatformKind::Linux => {
                    let direct = path.join("code");
                    if direct.is_file() {
                        direct
                    } else {
                        path.join("bin/code")
                    }
                }
                PlatformKind::MacOs => {
                    if path.extension().is_some_and(|extension| extension == "app") {
                        path.join("Contents/Resources/app/bin/code")
                    } else {
                        path.join("code")
                    }
                }
            };
        }
        if self.platform == PlatformKind::Windows
            && path
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("code.cmd"))
            && path
                .parent()
                .and_then(Path::parent)
                .is_some_and(|parent| parent.join("Code.exe").is_file())
        {
            return path.parent().unwrap().parent().unwrap().join("Code.exe");
        }
        path.to_owned()
    }

    fn product_candidates(&self, executable: &Path) -> Vec<PathBuf> {
        let mut products = Vec::new();
        let mut executable_forms = vec![executable.to_owned()];
        if let Ok(canonical) = fs::canonicalize(executable) {
            executable_forms.push(canonical);
        }
        for executable in executable_forms {
            let Some(directory) = executable.parent() else {
                continue;
            };
            match self.platform {
                PlatformKind::Windows => {
                    products.push(directory.join("resources/app/product.json"));
                    products.extend(versioned_product_candidates(directory));
                }
                PlatformKind::Linux => {
                    products.push(directory.join("resources/app/product.json"));
                    if let Some(parent) = directory.parent() {
                        products.push(parent.join("resources/app/product.json"));
                    }
                }
                PlatformKind::MacOs => {
                    if let Some(app) = directory.parent() {
                        products.push(app.join("product.json"));
                    }
                }
            }
        }
        unique_paths(products, self.target_identity_policy())
    }

    fn legacy_storage_roots(&self, name_short: &str) -> Vec<PathBuf> {
        let mut roots = vec![self.config_root.join(name_short)];
        if self.platform == PlatformKind::Linux {
            roots.push(self.home.join("snap/code/current/.config").join(name_short));
            roots.push(
                self.home
                    .join(".var/app/com.visualstudio.code/config")
                    .join(name_short),
            );
        }
        unique_paths(roots, self.target_identity_policy())
    }
}

fn versioned_product_candidates(directory: &Path) -> Vec<PathBuf> {
    let mut products = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let product = entry.path().join("resources/app/product.json");
            product.is_file().then(|| {
                let modified = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok();
                (modified, product)
            })
        })
        .collect::<Vec<_>>();
    products.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    products.into_iter().map(|(_, product)| product).collect()
}

fn unique_paths(paths: Vec<PathBuf>, identity_policy: TargetIdentityPolicy) -> Vec<PathBuf> {
    let mut seen = HashSet::with_capacity(paths.len());
    paths
        .into_iter()
        .filter(|path| seen.insert(path_key(path, identity_policy)))
        .collect()
}

fn path_key(path: &Path, identity_policy: TargetIdentityPolicy) -> String {
    let mut value = path.to_string_lossy().replace('\\', "/");
    if matches!(identity_policy, TargetIdentityPolicy::CaseInsensitiveAscii) {
        value.make_ascii_lowercase();
    }
    value
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VsCodeInstallation {
    pub code_exe: PathBuf,
    pub product_json: PathBuf,
    pub version: String,
    pub name_short: String,
    pub shared_data_folder_name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum VsCodeError {
    #[error("VS Code stable installation was not found")]
    NotFound,
    #[error("invalid VS Code product metadata: {0}")]
    InvalidProduct(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum RecentFormatError {
    #[error("recent value is invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("recent value has an unsupported root shape")]
    UnsupportedRoot,
}

pub fn discover_vscode(
    environment: &VsCodePlatformLayout,
    override_path: Option<&Path>,
) -> Result<VsCodeInstallation, VsCodeError> {
    if let Some(override_path) = override_path {
        let executable = environment.normalize_executable_candidate(override_path);
        return installation_from_executable(environment, &executable);
    }

    let mut seen = HashSet::new();
    for candidate in environment.executable_candidates() {
        let key = path_key(&candidate, environment.target_identity_policy());
        if !seen.insert(key) || !candidate.is_file() {
            continue;
        }
        if let Ok(installation) = installation_from_executable(environment, &candidate) {
            return Ok(installation);
        }
    }
    Err(VsCodeError::NotFound)
}

pub fn database_candidates(
    installation: &VsCodeInstallation,
    environment: &VsCodePlatformLayout,
) -> Vec<PathBuf> {
    let mut candidates = vec![
        environment
            .home
            .join(&installation.shared_data_folder_name)
            .join("sharedStorage/state.vscdb"),
    ];
    candidates.extend(
        environment
            .legacy_storage_roots(&installation.name_short)
            .into_iter()
            .map(|root| root.join("User/globalStorage/state.vscdb")),
    );
    unique_paths(candidates, environment.target_identity_policy())
}

pub fn window_state_candidates(
    installation: &VsCodeInstallation,
    environment: &VsCodePlatformLayout,
) -> Vec<PathBuf> {
    unique_paths(
        environment
            .legacy_storage_roots(&installation.name_short)
            .into_iter()
            .map(|root| root.join("User/globalStorage/storage.json"))
            .collect(),
        environment.target_identity_policy(),
    )
}

pub fn parse_recent_value(value: &str) -> Result<Vec<RecentProject>, RecentFormatError> {
    parse_recent_value_with_policy(value, TargetIdentityPolicy::current())
}

pub fn parse_recent_value_with_policy(
    value: &str,
    policy: TargetIdentityPolicy,
) -> Result<Vec<RecentProject>, RecentFormatError> {
    let parsed: Value = serde_json::from_str(value)?;
    let entries = match &parsed {
        Value::Object(object) => object
            .get("entries")
            .and_then(Value::as_array)
            .ok_or(RecentFormatError::UnsupportedRoot)?,
        Value::Array(entries) => entries,
        _ => return Err(RecentFormatError::UnsupportedRoot),
    };

    let projects = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| recent_project(entry, index as u32, policy))
        .collect();
    Ok(deduplicate_with_policy(projects, policy))
}

pub struct VsCodeRecentProvider {
    pub environment: DiscoveryEnvironment,
    pub override_path: Option<PathBuf>,
}

impl RecentProjectProvider for VsCodeRecentProvider {
    fn discover(&self) -> ProviderReport<Vec<RecentProject>> {
        let installation = match discover_vscode(&self.environment, self.override_path.as_deref()) {
            Ok(installation) => installation,
            Err(error) => {
                return ProviderReport {
                    value: Vec::new(),
                    diagnostics: vec![ProviderDiagnostic {
                        level: DiagnosticLevel::Error,
                        code: "vscode_not_found",
                        message: error.to_string(),
                    }],
                };
            }
        };
        let candidates = database_candidates(&installation, &self.environment);
        let mut diagnostics = Vec::new();
        for (index, database) in candidates.iter().enumerate() {
            if !database.is_file() {
                continue;
            }
            match read_recent_value(database) {
                Ok(Some(value)) => match parse_recent_value_with_policy(
                    &value,
                    self.environment.target_identity_policy(),
                ) {
                    Ok(projects) => {
                        diagnostics.push(ProviderDiagnostic {
                            level: DiagnosticLevel::Info,
                            code: "vscode_recent_loaded",
                            message: format!(
                                "loaded {} projects from db#{}",
                                projects.len(),
                                path_fingerprint(database)
                            ),
                        });
                        return ProviderReport {
                            value: projects,
                            diagnostics,
                        };
                    }
                    Err(error) => {
                        diagnostics.push(ProviderDiagnostic {
                            level: DiagnosticLevel::Error,
                            code: "vscode_recent_format_changed",
                            message: format!("db#{}: {error}", path_fingerprint(database)),
                        });
                        if index == 0 {
                            return ProviderReport {
                                value: Vec::new(),
                                diagnostics,
                            };
                        }
                    }
                },
                Ok(None) => diagnostics.push(ProviderDiagnostic {
                    level: DiagnosticLevel::Info,
                    code: "vscode_recent_key_missing",
                    message: format!("db#{} has no recent key", path_fingerprint(database)),
                }),
                Err(error) => {
                    diagnostics.push(ProviderDiagnostic {
                        level: DiagnosticLevel::Error,
                        code: "vscode_database_read_failed",
                        message: format!("db#{}: {error}", path_fingerprint(database)),
                    });
                    if index == 0 {
                        return ProviderReport {
                            value: Vec::new(),
                            diagnostics,
                        };
                    }
                }
            }
        }
        if diagnostics.is_empty() {
            diagnostics.push(ProviderDiagnostic {
                level: DiagnosticLevel::Warning,
                code: "vscode_database_not_found",
                message: "no VS Code recent database was found".to_owned(),
            });
        }
        ProviderReport {
            value: Vec::new(),
            diagnostics,
        }
    }
}

fn installation_from_executable(
    layout: &VsCodePlatformLayout,
    executable: &Path,
) -> Result<VsCodeInstallation, VsCodeError> {
    if !executable.is_file() {
        return Err(VsCodeError::NotFound);
    }
    if layout.platform == PlatformKind::Windows
        && !executable
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err(VsCodeError::InvalidProduct(
            "choose Code.exe; Windows batch scripts are not launched".to_owned(),
        ));
    }
    for product_json in layout.product_candidates(executable) {
        if !product_json.is_file() {
            continue;
        }
        let value: Value = serde_json::from_slice(&fs::read(&product_json)?)?;
        let application_name = value
            .get("applicationName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if application_name != "code" {
            continue;
        }
        let required = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| VsCodeError::InvalidProduct(format!("missing {name}")))
        };
        return Ok(VsCodeInstallation {
            code_exe: executable.to_owned(),
            product_json,
            version: required("version")?,
            name_short: required("nameShort")?,
            shared_data_folder_name: required("sharedDataFolderName")?,
        });
    }
    Err(VsCodeError::InvalidProduct(
        "stable product.json was not found for the executable".to_owned(),
    ))
}

fn read_recent_value(database: &Path) -> Result<Option<String>, String> {
    let connection = Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| error.to_string())?;
    connection
        .busy_timeout(Duration::from_millis(100))
        .map_err(|error| error.to_string())?;
    match connection.query_row(
        "SELECT value FROM ItemTable WHERE key = ?1",
        [RECENT_KEY],
        |row| match row.get_ref(0)? {
            ValueRef::Text(value) | ValueRef::Blob(value) => {
                Ok(String::from_utf8_lossy(value).into_owned())
            }
            _ => Err(rusqlite::Error::InvalidColumnType(
                0,
                "value".to_owned(),
                row.get_ref(0)?.data_type(),
            )),
        },
    ) {
        Ok(value) => Ok(Some(value)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn recent_project(
    entry: &Value,
    recent_index: u32,
    identity_policy: TargetIdentityPolicy,
) -> Option<RecentProject> {
    let object = entry.as_object()?;
    let label = object.get("label").and_then(Value::as_str);
    let (kind, raw_target) = if let Some(folder) = object.get("folderUri") {
        (ProjectKind::Folder, uri_from_value(folder)?)
    } else if let Some(workspace) = object.get("workspace") {
        let target = workspace
            .get("configPath")
            .and_then(uri_from_value)
            .or_else(|| workspace.get("uri").and_then(uri_from_value))
            .or_else(|| uri_from_value(workspace))?;
        (ProjectKind::Workspace, target)
    } else if let Some(workspace) = object.get("workspaceUri") {
        (ProjectKind::Workspace, uri_from_value(workspace)?)
    } else {
        let file = object.get("fileUri")?;
        let target = uri_from_value(file)?;
        if !is_workspace_target(&target) {
            return None;
        }
        (ProjectKind::Workspace, target)
    };

    let target = project_target(&raw_target)?;
    let fallback_name = target_name(kind, &target)?;
    let name = label
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(&fallback_name)
        .to_owned();
    let detail = match &target {
        ProjectTarget::LocalPath(path) => path.to_string_lossy().into_owned(),
        ProjectTarget::Uri(uri) => uri.clone(),
    };
    let key = target_key_with_policy(&target, identity_policy);
    Some(RecentProject {
        id: ProjectId(format!("vscode:{:016x}", fnv1a(key.as_bytes()))),
        provider: ProviderId("vscode".to_owned()),
        kind,
        target,
        name,
        detail,
        recent_index,
    })
}

fn is_workspace_target(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .map(|url| url.path().to_lowercase().ends_with(".code-workspace"))
        .unwrap_or_else(|| value.to_lowercase().ends_with(".code-workspace"))
}

fn uri_from_value(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        return Some(value.to_owned());
    }
    let object = value.as_object()?;
    if let Some(external) = object.get("external").and_then(Value::as_str) {
        return Some(external.to_owned());
    }
    let scheme = object.get("scheme").and_then(Value::as_str)?;
    let authority = object
        .get("authority")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = object
        .get("path")
        .or_else(|| object.get("fsPath"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .replace('\\', "/");
    let raw = if authority.is_empty() {
        format!("{scheme}://{path}")
    } else {
        format!("{scheme}://{authority}{path}")
    };
    Url::parse(&raw).ok().map(Into::into)
}

fn project_target(value: &str) -> Option<ProjectTarget> {
    if let Ok(url) = Url::parse(value) {
        if url.scheme() == "file" {
            if url
                .host_str()
                .is_some_and(|host| !host.eq_ignore_ascii_case("localhost"))
            {
                return None;
            }
            return url.to_file_path().ok().map(ProjectTarget::LocalPath);
        }
        if matches!(url.scheme(), "vscode-remote" | "vscode") {
            return Some(ProjectTarget::Uri(url.into()));
        }
        return None;
    }
    let path = PathBuf::from(value);
    path.is_absolute().then_some(ProjectTarget::LocalPath(path))
}

fn target_name(kind: ProjectKind, target: &ProjectTarget) -> Option<String> {
    let name = match target {
        ProjectTarget::LocalPath(path) => match kind {
            ProjectKind::Folder => path.file_name()?.to_string_lossy().into_owned(),
            ProjectKind::Workspace => path.file_stem()?.to_string_lossy().into_owned(),
        },
        ProjectTarget::Uri(value) => {
            let url = Url::parse(value).ok()?;
            let segment = url.path_segments()?.next_back()?.to_owned();
            match kind {
                ProjectKind::Folder => segment,
                ProjectKind::Workspace => segment
                    .strip_suffix(".code-workspace")
                    .unwrap_or(&segment)
                    .to_owned(),
            }
        }
    };
    (!name.is_empty()).then_some(name)
}

fn path_fingerprint(path: &Path) -> String {
    format!("{:016x}", fnv1a(path.to_string_lossy().as_bytes()))
}

fn fnv1a(value: &[u8]) -> u64 {
    value.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}
