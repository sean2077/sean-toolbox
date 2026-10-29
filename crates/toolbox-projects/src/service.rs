use std::path::Path;

use crate::{
    DiagnosticLevel, ProjectOpener, RecentProject, RecentProjectProvider, VsCodeOpener,
    VsCodePlatformLayout, VsCodeRecentProvider, discover_vscode, search_projects,
    window_state_candidates,
};

/// A display item retaining the provider's exact typed target for subsequent opening.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectItem {
    pub id: String,
    pub name: String,
    pub location: String,
    pub source: String,
    project: RecentProject,
}

impl From<RecentProject> for ProjectItem {
    fn from(project: RecentProject) -> Self {
        Self {
            id: project.id.0.clone(),
            name: project.name.clone(),
            location: project.detail.clone(),
            source: "VS Code".to_owned(),
            project,
        }
    }
}

/// Read VS Code's recent-project database once. Run this on a worker, not the UI loop.
pub fn discover(override_path: Option<&str>) -> Result<Vec<ProjectItem>, String> {
    let provider = VsCodeRecentProvider {
        environment: VsCodePlatformLayout::current(),
        override_path: override_path
            .filter(|path| !path.trim().is_empty())
            .map(Into::into),
    };
    let report = provider.discover();
    if report.value.is_empty() {
        let problems = report
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                matches!(
                    diagnostic.level,
                    DiagnosticLevel::Error | DiagnosticLevel::Warning
                )
            })
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
    }
    Ok(report.value.into_iter().map(Into::into).collect())
}

/// Rank cached items without disk access. An empty query preserves recency order.
pub fn search(items: &[ProjectItem], query: &str) -> Vec<ProjectItem> {
    let projects = items
        .iter()
        .map(|item| item.project.clone())
        .collect::<Vec<_>>();
    search_projects(&projects, query)
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Ask VS Code to open/focus a typed target, using separate argv entries throughout.
pub fn open(item: &ProjectItem, override_path: Option<&str>) -> Result<(), String> {
    let layout = VsCodePlatformLayout::current();
    let installation = discover_vscode(
        &layout,
        override_path
            .filter(|path| !path.trim().is_empty())
            .map(Path::new),
    )
    .map_err(|error| error.to_string())?;
    let opener = VsCodeOpener {
        window_state_files: window_state_candidates(&installation, &layout),
        executable: installation.code_exe,
    };
    opener
        .open_or_focus(&item.project)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use crate::{ProjectKind, ProjectTarget, parse_recent_value};
    use url::Url;

    use super::*;

    #[test]
    fn wrapper_preserves_workspace_and_remote_targets_through_search() {
        let directory = tempfile::tempdir().unwrap();
        let folder_uri = Url::from_directory_path(directory.path().join("local-project")).unwrap();
        let workspace_uri =
            Url::from_file_path(directory.path().join("team.code-workspace")).unwrap();
        let projects = parse_recent_value(
            &serde_json::json!({"entries": [
                {"folderUri": folder_uri.as_str()},
                {"workspace": {"configPath": workspace_uri.as_str()}},
                {"folderUri": "vscode-remote://ssh-remote+dev/work/api"}
            ]})
            .to_string(),
        )
        .unwrap();
        assert_eq!(projects.len(), 3);
        let items = projects
            .into_iter()
            .map(ProjectItem::from)
            .collect::<Vec<_>>();
        let workspace = search(&items, "team");
        assert_eq!(workspace.len(), 1);
        assert_eq!(workspace[0].project.kind, ProjectKind::Workspace);
        let remote = search(&items, "api");
        assert!(
            matches!(&remote[0].project.target, ProjectTarget::Uri(uri) if uri == "vscode-remote://ssh-remote+dev/work/api")
        );
        assert_eq!(search(&items, ""), items);
    }

    #[test]
    fn presentation_fields_never_replace_the_typed_launch_target() {
        let directory = tempfile::tempdir().unwrap();
        let folder_uri = Url::from_directory_path(directory.path().join("project")).unwrap();
        let original = parse_recent_value(
            &serde_json::json!({"entries": [{"folderUri": folder_uri.as_str()}]}).to_string(),
        )
        .unwrap()
        .remove(0);
        let mut item = ProjectItem::from(original.clone());
        item.location = "untrusted displayed text & executable".to_owned();
        assert_eq!(item.project.target, original.target);
    }
}
