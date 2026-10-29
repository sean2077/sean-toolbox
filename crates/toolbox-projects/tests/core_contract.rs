use std::{ffi::OsString, fs, path::PathBuf};

use toolbox_projects::{
    DiagnosticLevel, DiscoveryEnvironment, OpenOutcome, ProjectId, ProjectKind, ProjectTarget,
    ProviderId, RecentProject, RecentProjectProvider, TargetIdentityPolicy, VsCodeInstallation,
    VsCodePlatformLayout, VsCodeRecentProvider, build_launch_request, database_candidates,
    deduplicate_with_policy, discover_vscode, parse_recent_value, search_projects,
    window_state_candidates, window_state_targets,
};
#[cfg(windows)]
use toolbox_projects::{ProjectOpener, VsCodeOpener};
use url::Url;

fn project(name: &str, target: impl Into<PathBuf>, index: u32) -> RecentProject {
    let target = target.into();
    let detail = target.to_string_lossy().into_owned();
    RecentProject {
        id: ProjectId(format!("vscode:{index}")),
        provider: ProviderId("vscode".to_owned()),
        kind: ProjectKind::Folder,
        target: ProjectTarget::LocalPath(target),
        name: name.to_owned(),
        detail,
        recent_index: index,
    }
}

#[test]
fn parses_current_and_legacy_recent_shapes_and_excludes_plain_files() {
    let json = r#"{
        "entries": [
            {"folderUri":"file:///C:/work/recentry"},
            {"workspace":{"configPath":{"scheme":"file","path":"/C:/work/team.code-workspace"}}},
            {"fileUri":"file:///C:/work/notes.txt"},
            {"fileUri":"file:///C:/work/legacy.code-workspace?window=1#recent"},
            {"folderUri":{"scheme":"vscode-remote","authority":"ssh-remote+devbox","path":"/work/api"}}
        ]
    }"#;

    let projects = parse_recent_value(json).unwrap();
    assert_eq!(projects.len(), 4);
    assert_eq!(projects[0].name, "recentry");
    assert_eq!(projects[1].kind, ProjectKind::Workspace);
    assert_eq!(projects[2].name, "legacy");
    assert!(matches!(projects[3].target, ProjectTarget::Uri(_)));
    assert_eq!(projects[0].recent_index, 0);
    assert_eq!(projects[3].recent_index, 4);
}

#[test]
fn empty_search_preserves_recent_order_and_text_search_ranks_name_first() {
    let projects = vec![
        project("Other", r"C:\work\recentry-docs", 0),
        project("Recentry", r"C:\work\app", 1),
        project("recentry-tools", r"C:\work\tools", 2),
    ];
    assert_eq!(
        search_projects(&projects, "")
            .iter()
            .map(|item| item.recent_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(search_projects(&projects, "recentry")[0].name, "Recentry");
}

#[test]
fn deduplication_is_case_insensitive_for_windows_paths_and_keeps_newest() {
    let projects = vec![
        project("First", r"C:\Work\Recentry", 0),
        project("Duplicate", r"c:\work\recentry\", 1),
    ];
    let deduped = deduplicate_with_policy(projects, TargetIdentityPolicy::CaseInsensitiveAscii);
    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].name, "First");
}

#[test]
fn target_identity_respects_platform_case_semantics() {
    let projects = vec![
        project("Upper", "/work/Recentry", 0),
        project("Lower", "/work/recentry/", 1),
    ];

    assert_eq!(
        deduplicate_with_policy(projects.clone(), TargetIdentityPolicy::CaseSensitive).len(),
        2
    );
    let windows = deduplicate_with_policy(projects, TargetIdentityPolicy::CaseInsensitiveAscii);
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].name, "Upper");
}

#[test]
fn discovers_linux_stable_install_and_storage_layouts() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let config = home.join(".config");
    let data = home.join(".local/share");
    let app = directory.path().join("usr/share/code");
    let code = app.join("code");
    let product = app.join("resources/app/product.json");
    fs::create_dir_all(product.parent().unwrap()).unwrap();
    fs::write(&code, b"").unwrap();
    fs::write(
        &product,
        r#"{"applicationName":"code","nameShort":"Code","version":"1.130.0","sharedDataFolderName":".vscode"}"#,
    )
    .unwrap();
    let layout =
        VsCodePlatformLayout::linux(home.clone(), config.clone(), data, vec![app], Vec::new());

    let installation = discover_vscode(&layout, None).unwrap();
    assert_eq!(installation.code_exe, code);
    assert_eq!(installation.product_json, product);
    assert_eq!(
        database_candidates(&installation, &layout),
        vec![
            home.join(".vscode/sharedStorage/state.vscdb"),
            config.join("Code/User/globalStorage/state.vscdb"),
            home.join("snap/code/current/.config/Code/User/globalStorage/state.vscdb"),
            home.join(".var/app/com.visualstudio.code/config/Code/User/globalStorage/state.vscdb"),
        ]
    );
    assert_eq!(
        window_state_candidates(&installation, &layout),
        vec![
            config.join("Code/User/globalStorage/storage.json"),
            home.join("snap/code/current/.config/Code/User/globalStorage/storage.json"),
            home.join(".var/app/com.visualstudio.code/config/Code/User/globalStorage/storage.json"),
        ]
    );
}

#[test]
fn discovers_macos_stable_app_and_storage_layouts() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let application_support = home.join("Library/Application Support");
    let bundle = directory.path().join("Applications/Visual Studio Code.app");
    let app = bundle.join("Contents/Resources/app");
    let code = app.join("bin/code");
    let product = app.join("product.json");
    fs::create_dir_all(code.parent().unwrap()).unwrap();
    fs::write(&code, b"").unwrap();
    fs::write(
        &product,
        r#"{"applicationName":"code","nameShort":"Code","version":"1.130.0","sharedDataFolderName":".vscode"}"#,
    )
    .unwrap();
    let layout = VsCodePlatformLayout::macos(
        home.clone(),
        application_support.clone(),
        vec![bundle],
        Vec::new(),
    );

    let installation = discover_vscode(&layout, None).unwrap();
    assert_eq!(installation.code_exe, code);
    assert_eq!(installation.product_json, product);
    assert_eq!(
        database_candidates(&installation, &layout),
        vec![
            home.join(".vscode/sharedStorage/state.vscdb"),
            application_support.join("Code/User/globalStorage/state.vscdb"),
        ]
    );
    assert_eq!(
        window_state_candidates(&installation, &layout),
        vec![application_support.join("Code/User/globalStorage/storage.json")]
    );
}

#[test]
fn shared_database_precedes_legacy_global_storage() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let app_data = directory.path().join("appdata");
    let environment = DiscoveryEnvironment::windows(
        home.clone(),
        app_data.clone(),
        directory.path().join("local-appdata"),
        Vec::new(),
        Vec::new(),
    );
    let installation = VsCodeInstallation {
        code_exe: directory.path().join("Code/Code.exe"),
        product_json: directory.path().join("Code/resources/app/product.json"),
        version: "1.129.1".to_owned(),
        name_short: "Code".to_owned(),
        shared_data_folder_name: ".vscode-shared".to_owned(),
    };
    let candidates = database_candidates(&installation, &environment);
    assert_eq!(
        candidates,
        vec![
            home.join(".vscode-shared")
                .join("sharedStorage")
                .join("state.vscdb"),
            app_data
                .join("Code")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
        ]
    );
}

#[test]
fn launch_arguments_focus_open_folder_and_create_new_workspace_window() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("Code.exe");
    let folder_path = directory.path().join("work/recentry");
    let folder = project("Recentry", folder_path.clone(), 0);
    let folder_uri: String = Url::from_directory_path(&folder_path).unwrap().into();
    let folder_request = build_launch_request(executable.clone(), &folder, &[folder_uri]).unwrap();
    assert_eq!(folder_request.outcome, OpenOutcome::Focused);
    assert!(
        !folder_request
            .args
            .contains(&OsString::from("--new-window"))
    );
    assert_eq!(folder_request.args[0], "--folder-uri");

    let workspace_path = directory.path().join("work/team.code-workspace");
    let workspace = RecentProject {
        kind: ProjectKind::Workspace,
        target: ProjectTarget::LocalPath(workspace_path.clone()),
        ..project("team", workspace_path, 1)
    };
    let workspace_request = build_launch_request(executable.clone(), &workspace, &[]).unwrap();
    assert_eq!(workspace_request.outcome, OpenOutcome::OpenedNew);
    assert_eq!(workspace_request.args[0], "--new-window");
    assert_eq!(workspace_request.args[1], "--file-uri");

    let remote = RecentProject {
        target: ProjectTarget::Uri("vscode-remote://ssh-remote+devbox/work/api".to_owned()),
        ..project("api", directory.path().join("unused"), 2)
    };
    let remote_request = build_launch_request(executable.clone(), &remote, &[]).unwrap();
    assert_eq!(remote_request.args[1], "--folder-uri");
    assert_eq!(
        remote_request.args[2],
        "vscode-remote://ssh-remote+devbox/work/api"
    );

    let unsupported = RecentProject {
        target: ProjectTarget::Uri("https://example.com/project".to_owned()),
        ..project("unsafe", directory.path().join("unused"), 3)
    };
    assert!(build_launch_request(executable, &unsupported, &[]).is_err());
}

#[cfg(windows)]
#[test]
fn opener_starts_a_fake_code_executable_and_reports_spawn_failure() {
    let directory = tempfile::tempdir().unwrap();
    let fake_code = directory.path().join("code.exe");
    let system_root = std::env::var_os("SystemRoot").unwrap();
    fs::copy(
        PathBuf::from(system_root).join("System32/where.exe"),
        &fake_code,
    )
    .unwrap();
    let opener = VsCodeOpener {
        executable: fake_code,
        window_state_files: Vec::new(),
    };
    assert_eq!(
        opener
            .open_or_focus(&project("Recentry", r"C:\work\recentry", 0))
            .unwrap(),
        OpenOutcome::OpenedNew
    );

    let missing = VsCodeOpener {
        executable: directory.path().join("missing-code.exe"),
        window_state_files: Vec::new(),
    };
    assert!(
        missing
            .open_or_focus(&project("Recentry", r"C:\work\recentry", 0))
            .is_err()
    );
}

#[test]
fn window_state_extracts_folder_and_workspace_targets() {
    let value = serde_json::json!({
        "windowsState": {
            "lastActiveWindow": {"folder": "file:///C:/work/recentry"},
            "openedWindows": [
                {"workspaceIdentifier": {"configURIPath": "file:///C:/work/team.code-workspace"}}
            ]
        }
    });
    assert_eq!(
        window_state_targets(&value),
        vec![
            "file:///C:/work/recentry".to_owned(),
            "file:///C:/work/team.code-workspace".to_owned()
        ]
    );
}

#[test]
fn malformed_sqlite_is_a_safe_provider_failure() {
    let directory = tempfile::tempdir().unwrap();
    let code = directory.path().join("Code.exe");
    fs::write(&code, b"").unwrap();
    let product = directory.path().join("resources/app/product.json");
    fs::create_dir_all(product.parent().unwrap()).unwrap();
    fs::write(
        &product,
        r#"{"applicationName":"code","nameShort":"Code","version":"1.0.0","sharedDataFolderName":".vscode-shared"}"#,
    )
    .unwrap();
    let database = directory
        .path()
        .join("home/.vscode-shared/sharedStorage/state.vscdb");
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    fs::write(&database, b"not a sqlite database").unwrap();
    let provider = VsCodeRecentProvider {
        environment: DiscoveryEnvironment::windows(
            directory.path().join("home"),
            directory.path().join("appdata"),
            directory.path().join("local"),
            Vec::new(),
            Vec::new(),
        ),
        override_path: Some(code),
    };
    let report = provider.discover();
    assert!(report.value.is_empty());
    assert_eq!(report.diagnostics[0].level, DiagnosticLevel::Error);
    assert_eq!(report.diagnostics[0].code, "vscode_database_read_failed");
    assert!(
        !report.diagnostics[0]
            .message
            .contains(directory.path().to_string_lossy().as_ref())
    );
}

#[test]
fn discovers_current_versioned_windows_install_layout() {
    let directory = tempfile::tempdir().unwrap();
    let code = directory.path().join("Microsoft VS Code/Code.exe");
    fs::create_dir_all(code.parent().unwrap()).unwrap();
    fs::write(&code, b"").unwrap();
    let product = directory
        .path()
        .join("Microsoft VS Code/abc123/resources/app/product.json");
    fs::create_dir_all(product.parent().unwrap()).unwrap();
    fs::write(
        &product,
        r#"{"applicationName":"code","nameShort":"Code","version":"1.129.1","sharedDataFolderName":".vscode-shared"}"#,
    )
    .unwrap();
    let environment = DiscoveryEnvironment::windows(
        directory.path().join("home"),
        directory.path().join("appdata"),
        directory.path().join("local"),
        vec![directory.path().join("Microsoft VS Code")],
        Vec::new(),
    );
    let installation = discover_vscode(&environment, Some(&code)).unwrap();
    assert_eq!(installation.product_json, product);
    assert_eq!(installation.version, "1.129.1");
}
