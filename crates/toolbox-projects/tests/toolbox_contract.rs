use std::{fs, path::PathBuf};

use toolbox_projects::{
    DiagnosticLevel, ProjectKind, ProjectTarget, RecentProjectProvider, VsCodePlatformLayout,
    VsCodeRecentProvider, build_launch_request, discover_vscode, parse_recent_value,
};
use url::Url;

#[test]
fn provider_reads_database_without_changing_its_contents() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let app = directory.path().join("code");
    fs::create_dir_all(app.join("resources/app")).unwrap();
    fs::write(app.join("code"), b"").unwrap();
    fs::write(app.join("resources/app/product.json"), r#"{"applicationName":"code","nameShort":"Code","version":"1.0.0","sharedDataFolderName":".vscode"}"#).unwrap();
    let database = home.join(".vscode/sharedStorage/state.vscdb");
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    let folder_uri = Url::from_directory_path(directory.path().join("project")).unwrap();
    let recent_value =
        serde_json::json!({"entries": [{"folderUri": folder_uri.as_str()}]}).to_string();
    {
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch("CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                [toolbox_projects::RECENT_KEY, recent_value.as_str()],
            )
            .unwrap();
    }
    let before = fs::read(&database).unwrap();
    let provider = VsCodeRecentProvider {
        environment: VsCodePlatformLayout::linux(
            home.clone(),
            home.join(".config"),
            home.join(".local/share"),
            vec![app],
            vec![],
        ),
        override_path: None,
    };
    let report = provider.discover();
    assert_eq!(report.value.len(), 1);
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| d.level == DiagnosticLevel::Info)
    );
    assert_eq!(fs::read(&database).unwrap(), before);
}

#[test]
fn windows_override_resolves_code_cmd_to_real_exe_or_rejects_it() {
    let directory = tempfile::tempdir().unwrap();
    let app = directory.path().join("app");
    fs::create_dir_all(app.join("bin")).unwrap();
    fs::create_dir_all(app.join("resources/app")).unwrap();
    let script = app.join("bin/code.cmd");
    fs::write(&script, b"@echo off").unwrap();
    fs::write(app.join("resources/app/product.json"), r#"{"applicationName":"code","nameShort":"Code","version":"1.0.0","sharedDataFolderName":".vscode"}"#).unwrap();
    let layout = VsCodePlatformLayout::windows(
        directory.path().to_owned(),
        directory.path().to_owned(),
        directory.path().to_owned(),
        vec![],
        vec![],
    );
    let error = discover_vscode(&layout, Some(&script)).unwrap_err();
    assert!(error.to_string().contains("batch scripts"));
    let executable = app.join("Code.exe");
    fs::write(&executable, b"").unwrap();
    assert_eq!(
        discover_vscode(&layout, Some(&script)).unwrap().code_exe,
        executable
    );
}

#[test]
fn remote_uri_with_shell_punctuation_remains_one_argument() {
    let projects = parse_recent_value(r#"{"entries":[{"workspace":{"configPath":"vscode-remote://ssh-remote+dev/work/team%20%26%20project.code-workspace"}}]}"#).unwrap();
    assert_eq!(projects[0].kind, ProjectKind::Workspace);
    let ProjectTarget::Uri(uri) = &projects[0].target else {
        panic!("remote target lost");
    };
    let request = build_launch_request(PathBuf::from("Code.exe"), &projects[0], &[]).unwrap();
    assert_eq!(request.args.len(), 3);
    assert_eq!(request.args[1], "--file-uri");
    assert_eq!(request.args[2], uri.as_str());
}

#[test]
fn ignores_unrelated_url_schemes_and_preserves_target_identity_when_label_changes() {
    let directory = tempfile::tempdir().unwrap();
    let folder_uri = Url::from_directory_path(directory.path().join("project")).unwrap();
    let first = parse_recent_value(
        &serde_json::json!({"entries": [
            {"folderUri": "https://example.com"},
            {"folderUri": folder_uri.as_str(), "label": "Old name"}
        ]})
        .to_string(),
    )
    .unwrap();
    let second = parse_recent_value(
        &serde_json::json!({"entries": [{"folderUri": folder_uri.as_str(), "label": "New name"}]})
            .to_string(),
    )
    .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, second[0].id);
    assert_ne!(first[0].name, second[0].name);
}

#[test]
fn uri_components_preserve_project_names_with_reserved_characters() {
    let directory = tempfile::tempdir().unwrap();
    let first_path = directory.path().join("C# notes%2F25");
    let second_path = directory.path().join("C# drafts%2F25");
    let component = |path: &std::path::Path| {
        serde_json::json!({"folderUri": {
            "scheme": "file",
            "path": path.to_string_lossy().replace('\\', "/")
        }})
    };
    let projects = parse_recent_value(
        &serde_json::json!({"entries": [component(&first_path), component(&second_path)]})
            .to_string(),
    )
    .unwrap();
    assert_eq!(projects.len(), 2, "distinct names must not collapse at '#'");
    assert_eq!(
        projects[0].target,
        ProjectTarget::LocalPath(first_path.clone())
    );
    assert_eq!(projects[1].target, ProjectTarget::LocalPath(second_path));
    assert_eq!(projects[0].name, "C# notes%2F25");
    let expected_uri = Url::from_directory_path(&first_path).unwrap();
    let request = build_launch_request(PathBuf::from("code"), &projects[0], &[]).unwrap();
    assert_eq!(request.args[2], expected_uri.as_str());

    let string_form = parse_recent_value(
        &serde_json::json!({"entries": [{"folderUri": expected_uri.as_str()}]}).to_string(),
    )
    .unwrap();
    assert_eq!(projects[0].id, string_form[0].id);
}

#[test]
fn remote_names_are_readable_without_changing_the_launch_uri() {
    let projects = parse_recent_value(
        &serde_json::json!({"entries": [{"folderUri": {
            "scheme": "vscode-remote",
            "authority": "ssh-remote+dev",
            "path": "/work/项目 + C#%2F?",
            "query": "window=1",
            "fragment": "recent"
        }}]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "项目 + C#%2F?");
    let uri =
        "vscode-remote://ssh-remote+dev/work/%E9%A1%B9%E7%9B%AE%20+%20C%23%252F%3F?window=1#recent";
    assert_eq!(projects[0].target, ProjectTarget::Uri(uri.into()));
    let request = build_launch_request(PathBuf::from("code"), &projects[0], &[]).unwrap();
    assert_eq!(request.args[2], uri);
}

#[cfg(unix)]
#[test]
fn uri_component_backslashes_are_literal_unix_filename_characters() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("project\\notes");
    let projects = parse_recent_value(
        &serde_json::json!({"entries": [{"folderUri": {
            "scheme": "file",
            "path": path.to_string_lossy()
        }}]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].target, ProjectTarget::LocalPath(path));
    assert_eq!(projects[0].name, "project\\notes");
}

#[test]
fn root_and_remote_trailing_slash_folders_remain_searchable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().ancestors().last().unwrap();
    let root_uri = Url::from_directory_path(root).unwrap();
    let projects = parse_recent_value(
        &serde_json::json!({"entries": [
            {"folderUri": root_uri.as_str()},
            {"folderUri": "vscode-remote://ssh-remote+dev/"},
            {"folderUri": "vscode-remote://ssh-remote+dev/work/team%20project/"}
        ]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(projects.len(), 3);
    assert_eq!(projects[0].name, root.to_string_lossy());
    assert_eq!(projects[1].name, "ssh-remote+dev");
    assert_eq!(projects[2].name, "team project");
    assert_eq!(
        toolbox_projects::search_projects(&projects, "team project").len(),
        1
    );
}
