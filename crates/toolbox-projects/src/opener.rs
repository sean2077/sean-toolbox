use std::{ffi::OsString, fs, path::PathBuf, process::Command};

use serde_json::Value;
use url::Url;

use crate::{OpenOutcome, ProjectKind, ProjectTarget, RecentProject, target_key};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub outcome: OpenOutcome,
}

pub fn window_state_targets(value: &Value) -> Vec<String> {
    let Some(windows_state) = value.get("windowsState").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    if let Some(last) = windows_state.get("lastActiveWindow") {
        windows.push(last);
    }
    if let Some(opened) = windows_state.get("openedWindows").and_then(Value::as_array) {
        windows.extend(opened.iter());
    }
    windows
        .into_iter()
        .filter_map(|window| {
            window
                .get("folder")
                .and_then(Value::as_str)
                .or_else(|| {
                    window
                        .get("workspaceIdentifier")
                        .and_then(|workspace| workspace.get("configURIPath"))
                        .and_then(Value::as_str)
                })
                .map(str::to_owned)
        })
        .collect()
}

pub fn build_launch_request(
    executable: PathBuf,
    project: &RecentProject,
    open_targets: &[String],
) -> Result<LaunchRequest, OpenError> {
    let target_uri = target_uri(project)?;
    let wanted_key = target_key(&project.target);
    let target_is_open = open_targets.iter().any(|target| {
        comparable_target(target).is_some_and(|candidate| target_key(&candidate) == wanted_key)
    });
    let mut args = Vec::with_capacity(3);
    if !target_is_open {
        args.push(OsString::from("--new-window"));
    }
    args.push(OsString::from(target_argument_kind(
        project.kind,
        &project.target,
    )));
    args.push(OsString::from(target_uri));
    Ok(LaunchRequest {
        executable,
        args,
        outcome: if target_is_open {
            OpenOutcome::Focused
        } else {
            OpenOutcome::OpenedNew
        },
    })
}

#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error("unsupported target: {0}")]
    UnsupportedTarget(String),
    #[error("unsupported executable; choose Code.exe instead of a batch script: {0}")]
    UnsupportedExecutable(String),
    #[error("failed to start VS Code: {0}")]
    Spawn(#[from] std::io::Error),
}

pub struct VsCodeOpener {
    pub executable: PathBuf,
    pub window_state_files: Vec<PathBuf>,
}

impl crate::ProjectOpener for VsCodeOpener {
    type Error = OpenError;

    fn open_or_focus(&self, project: &RecentProject) -> Result<OpenOutcome, Self::Error> {
        // Rust's Windows Command implementation may implicitly invoke cmd.exe for
        // batch files. Only a native executable is permitted on that platform.
        #[cfg(windows)]
        if !self
            .executable
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
        {
            return Err(OpenError::UnsupportedExecutable(
                self.executable.to_string_lossy().into_owned(),
            ));
        }
        let open_targets = self
            .window_state_files
            .iter()
            .filter_map(|path| fs::read(path).ok())
            .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .flat_map(|value| window_state_targets(&value))
            .collect::<Vec<_>>();
        let request = build_launch_request(self.executable.clone(), project, &open_targets)?;
        let mut child = Command::new(&request.executable)
            .args(&request.args)
            .spawn()?;
        // Reap the CLI process after it forwards the request to an existing VS
        // Code instance. Dropping Child alone leaves zombies on Unix.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(request.outcome)
    }
}

fn target_uri(project: &RecentProject) -> Result<String, OpenError> {
    match &project.target {
        ProjectTarget::LocalPath(path) => {
            let url = match project.kind {
                ProjectKind::Folder => Url::from_directory_path(path),
                ProjectKind::Workspace => Url::from_file_path(path),
            }
            .map_err(|_| OpenError::UnsupportedTarget(path.to_string_lossy().into_owned()))?;
            Ok(url.into())
        }
        ProjectTarget::Uri(value) => {
            let url =
                Url::parse(value).map_err(|_| OpenError::UnsupportedTarget(value.to_owned()))?;
            if !matches!(url.scheme(), "vscode-remote" | "vscode") {
                return Err(OpenError::UnsupportedTarget(value.to_owned()));
            }
            Ok(url.into())
        }
    }
}

fn comparable_target(value: &str) -> Option<ProjectTarget> {
    if let Ok(url) = Url::parse(value) {
        if url.scheme() == "file" {
            return url.to_file_path().ok().map(ProjectTarget::LocalPath);
        }
        if matches!(url.scheme(), "vscode-remote" | "vscode") {
            return Some(ProjectTarget::Uri(url.into()));
        }
    }
    let path = PathBuf::from(value);
    path.is_absolute().then_some(ProjectTarget::LocalPath(path))
}

pub fn target_argument_kind(kind: ProjectKind, target: &ProjectTarget) -> &'static str {
    match (kind, target) {
        (ProjectKind::Folder, _) => "--folder-uri",
        (ProjectKind::Workspace, _) => "--file-uri",
    }
}
