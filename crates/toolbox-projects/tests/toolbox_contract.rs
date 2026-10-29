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
