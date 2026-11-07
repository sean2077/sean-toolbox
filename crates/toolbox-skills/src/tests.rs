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
        // Windows CI cannot assume the symlink privilege, so cross-platform
        // tests exercise the copy path; Unix tests cover the symlink default.
        #[cfg(windows)]
        fixture.set_mode(SkillDeployMode::Copy);
        fixture
    }

    fn set_mode(&mut self, mode: SkillDeployMode) {
        self.service()
            .execute(SkillsCommand::SetDeployMode { mode }, 0)
            .unwrap();
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
fn batch_enable_is_authoritative_for_every_selected_skill() {
    let mut fixture = Fixture::new();
    fixture.commit("alpha", "alpha");
    fixture.commit("beta", "beta");
    fixture.install("alpha", vec![SkillApp::Claude], 10);
    fixture.install("beta", vec![SkillApp::Claude], 10);

    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabledBatch {
                skill_ids: vec!["alpha".into(), "beta".into(), "alpha".into()],
                app: SkillApp::Codex,
                enabled: true,
            },
            11,
        )
        .unwrap();
    for name in ["alpha", "beta"] {
        assert!(fixture.app(SkillApp::Codex, name).exists());
    }
    let snapshot = fixture.service().snapshot();
    for name in ["alpha", "beta"] {
        let skill = snapshot
            .installed
            .iter()
            .find(|skill| skill.id == name)
            .unwrap();
        assert_eq!(skill.apps, vec![SkillApp::Claude, SkillApp::Codex]);
    }

    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabledBatch {
                skill_ids: vec!["alpha".into(), "beta".into()],
                app: SkillApp::Codex,
                enabled: false,
            },
            12,
        )
        .unwrap();
    for name in ["alpha", "beta"] {
        assert!(!fixture.app(SkillApp::Codex, name).exists());
        assert!(fixture.app(SkillApp::Claude, name).exists());
    }

    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabledBatch {
                skill_ids: vec!["alpha".into(), "beta".into()],
                app: SkillApp::Claude,
                enabled: false,
            },
            13,
        )
        .unwrap();
    let snapshot = fixture.service().snapshot();
    for name in ["alpha", "beta"] {
        let skill = snapshot
            .installed
            .iter()
            .find(|skill| skill.id == name)
            .unwrap();
        assert!(skill.apps.is_empty());
        assert!(!fixture.app(SkillApp::Claude, name).exists());
    }
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
    fixture.set_mode(SkillDeployMode::Copy);
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
fn deploy_mode_defaults_to_symlink_and_persists_the_choice() {
    assert_eq!(State::default().deploy_mode, SkillDeployMode::Symlink);
    let mut fixture = Fixture::new();
    fixture.set_mode(SkillDeployMode::Copy);
    fixture.reopen();
    assert_eq!(
        fixture.service().snapshot().deploy_mode,
        SkillDeployMode::Copy
    );
}

#[cfg(unix)]
#[test]
fn managed_skills_link_app_directories_by_default() {
    let mut fixture = Fixture::new();
    fixture.set_mode(SkillDeployMode::Symlink);
    fixture.commit("review", "linked");
    fixture.install("review", vec![SkillApp::Claude], 1);
    let master = fixture.master("review");
    let link = fixture.app(SkillApp::Claude, "review");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(files::links_to(&link, &master).unwrap());
    assert_eq!(file_content(&link), "linked");

    // An update replaces the master in place; app links keep following it.
    fixture.commit("review", "linked v2");
    fixture
        .service()
        .execute(SkillsCommand::Update { skill_id: None }, 2)
        .unwrap();
    assert_eq!(file_content(&link), "linked v2");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabled {
                skill_id: "review".into(),
                app: SkillApp::Claude,
                enabled: false,
            },
            3,
        )
        .unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    assert_eq!(file_content(&master), "linked v2");

    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabled {
                skill_id: "review".into(),
                app: SkillApp::Claude,
                enabled: true,
            },
            4,
        )
        .unwrap();
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "review".into(),
            },
            5,
        )
        .unwrap();
    assert!(!master.exists());
    assert!(fs::symlink_metadata(&link).is_err());
}

#[cfg(unix)]
#[test]
fn switching_to_copy_changes_new_enables_and_mixed_deployments_uninstall() {
    let mut fixture = Fixture::new();
    fixture.set_mode(SkillDeployMode::Symlink);
    fixture.commit("review", "mixed");
    fixture.install("review", vec![SkillApp::Claude], 1);
    let claude = fixture.app(SkillApp::Claude, "review");
    assert!(
        fs::symlink_metadata(&claude)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    fixture.set_mode(SkillDeployMode::Copy);
    // Existing links stay as they are until the switch is cycled.
    assert!(
        fs::symlink_metadata(&claude)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabled {
                skill_id: "review".into(),
                app: SkillApp::Codex,
                enabled: true,
            },
            2,
        )
        .unwrap();
    let codex = fixture.app(SkillApp::Codex, "review");
    assert!(
        !fs::symlink_metadata(&codex)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(file_content(&codex), "mixed");

    // Re-enabling the linked app now deploys a copy.
    for enabled in [false, true] {
        fixture
            .service()
            .execute(
                SkillsCommand::SetEnabled {
                    skill_id: "review".into(),
                    app: SkillApp::Claude,
                    enabled,
                },
                3,
            )
            .unwrap();
    }
    assert!(
        !fs::symlink_metadata(&claude)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(file_content(&claude), "mixed");

    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "review".into(),
            },
            4,
        )
        .unwrap();
    assert!(!fixture.master("review").exists());
    assert!(fs::symlink_metadata(&claude).is_err());
    assert!(fs::symlink_metadata(&codex).is_err());
}

#[cfg(unix)]
#[test]
fn a_missing_master_copy_does_not_block_link_cleanup() {
    let mut fixture = Fixture::new();
    fixture.set_mode(SkillDeployMode::Symlink);
    fixture.commit("review", "cleanup");
    fixture.install("review", vec![SkillApp::Claude], 1);
    let link = fixture.app(SkillApp::Claude, "review");
    fs::remove_dir_all(fixture.master("review")).unwrap();

    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabled {
                skill_id: "review".into(),
                app: SkillApp::Claude,
                enabled: false,
            },
            2,
        )
        .unwrap();
    assert!(fs::symlink_metadata(&link).is_err());
    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "review".into(),
            },
            3,
        )
        .unwrap();
    assert!(fixture.service().snapshot().installed.is_empty());
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
fn disabling_a_removed_app_copy_does_not_recreate_its_directory() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    fixture.install("review", vec![SkillApp::Codex], 1);
    fs::remove_dir_all(fixture.home.join(".codex")).unwrap();
    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabled {
                skill_id: "review".into(),
                app: SkillApp::Codex,
                enabled: false,
            },
            2,
        )
        .unwrap();
    assert!(!fixture.home.join(".codex").exists());
    assert!(!fixture.app(SkillApp::Codex, "review").exists());
}

#[test]
fn uninstall_does_not_recreate_removed_app_directories() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    fixture.install("review", vec![SkillApp::Codex], 1);
    fs::remove_dir_all(fixture.home.join(".codex")).unwrap();
    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "review".into(),
            },
            2,
        )
        .unwrap();
    assert!(!fixture.home.join(".codex").exists());
    assert!(!fixture.master("review").exists());
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
                link: false,
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
fn local_scan_continues_after_one_root_is_not_a_directory() {
    let mut fixture = Fixture::new();
    fs::create_dir(fixture.home.join(".claude")).unwrap();
    fs::write(fixture.home.join(".claude/skills"), "not a directory").unwrap();
    let shared = fixture.home.join(".agents/skills/valid");
    fs::create_dir_all(&shared).unwrap();
    fs::write(
        shared.join("SKILL.md"),
        "---\nname: Shared\ndescription: shared\n---\n",
    )
    .unwrap();
    // The malformed root is reported, but the remaining roots are still scanned.
    assert!(
        fixture
            .service()
            .execute(SkillsCommand::ScanLocal, 0)
            .is_err()
    );
    let snapshot = fixture.service().snapshot();
    assert!(
        snapshot
            .local
            .iter()
            .any(|skill| skill.directory == "valid")
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
                    apps: vec![],
                    link: false,
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

#[test]
fn git_environment_names_are_scrubbed_case_insensitively() {
    use std::ffi::OsStr;
    assert!(git::is_git_environment(OsStr::new("GIT_DIR")));
    assert!(git::is_git_environment(OsStr::new("Git_Dir")));
    assert!(git::is_git_environment(OsStr::new("git_config_global")));
    assert!(!git::is_git_environment(OsStr::new("GIT")));
    assert!(!git::is_git_environment(OsStr::new("PATH")));
}

#[test]
fn an_indented_rule_inside_a_block_description_does_not_end_frontmatter() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(
        directory.path().join("SKILL.md"),
        "---\nname: Review\ndescription: |\n  First line\n  ---\n  Last line\n---\nBody\n",
    )
    .unwrap();
    let (name, description) = files::descriptor(directory.path(), "fallback").unwrap();
    assert_eq!(name, "Review");
    assert_eq!(description, "First line --- Last line");
}

#[test]
fn a_long_skill_body_still_yields_its_frontmatter() {
    let directory = tempfile::tempdir().unwrap();
    // A multi-byte body crossing the read limit must not make the file unreadable.
    let body = "参考".repeat(40 * 1024);
    fs::write(
        directory.path().join("SKILL.md"),
        format!("---\nname: claude-api\ndescription: |-\n  Reference\n---\n{body}"),
    )
    .unwrap();
    let (name, description) = files::descriptor(directory.path(), "fallback").unwrap();
    assert_eq!(name, "claude-api");
    assert_eq!(description, "Reference");
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
                link: false,
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
fn a_status_write_failure_does_not_report_a_completed_command_as_failed() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    fs::remove_file(fixture.root.join("state.json")).unwrap();
    fs::create_dir(fixture.root.join("state.json")).unwrap();
    let result = fixture.service().execute(SkillsCommand::Discover, 1);
    assert!(result.is_ok(), "{result:?}");
    let snapshot = fixture.service().snapshot();
    assert!(
        snapshot
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("Cannot save skills status")
    );
    assert_eq!(snapshot.available.len(), 1);
}

#[cfg(unix)]
#[test]
fn local_scan_lists_linked_folders_read_only() {
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    let target = fixture.home.join("vault/ob");
    fs::create_dir_all(&target).unwrap();
    fs::write(
        target.join("SKILL.md"),
        "---\nname: ob\ndescription: Linked fixture\n---\n",
    )
    .unwrap();
    let app_root = fixture.home.join(".claude/skills");
    fs::create_dir_all(&app_root).unwrap();
    symlink(&target, app_root.join("ob")).unwrap();
    fixture
        .service()
        .execute(SkillsCommand::ScanLocal, 0)
        .unwrap();
    let snapshot = fixture.service().snapshot();
    let linked = snapshot
        .local
        .iter()
        .find(|skill| skill.directory == "ob")
        .expect("linked folder is listed");
    assert!(linked.linked);
    assert_eq!(linked.app, Some(SkillApp::Claude));
    let expected = target
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(linked.target.as_deref(), Some(expected.as_str()));
}

#[cfg(unix)]
#[test]
fn linked_local_skills_deploy_links_without_copying_the_source() {
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    let source = fixture.home.join("vault/ob");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: ob\ndescription: Linked\n---\n",
    )
    .unwrap();
    let claude_root = fixture.home.join(".claude/skills");
    fs::create_dir_all(&claude_root).unwrap();
    symlink(&source, claude_root.join("ob")).unwrap();
    fixture
        .service()
        .execute(
            SkillsCommand::ImportLocal {
                path: source.to_string_lossy().into_owned(),
                apps: vec![SkillApp::Claude, SkillApp::Codex],
                link: true,
            },
            1,
        )
        .unwrap();
    let snapshot = fixture.service().snapshot();
    let skill = &snapshot.installed[0];
    assert!(skill.linked);
    assert_eq!(skill.apps, vec![SkillApp::Claude, SkillApp::Codex]);
    assert!(!fixture.master("ob").exists());
    let codex_link = fixture.home.join(".codex/skills/ob");
    assert!(
        fs::symlink_metadata(&codex_link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_link(&codex_link).unwrap(),
        source.canonicalize().unwrap()
    );

    fixture
        .service()
        .execute(
            SkillsCommand::SetEnabled {
                skill_id: "ob".into(),
                app: SkillApp::Codex,
                enabled: false,
            },
            2,
        )
        .unwrap();
    assert!(fs::symlink_metadata(&codex_link).is_err());
    assert!(source.join("SKILL.md").is_file());

    fixture
        .service()
        .execute(
            SkillsCommand::Uninstall {
                skill_id: "ob".into(),
            },
            3,
        )
        .unwrap();
    assert!(fs::symlink_metadata(claude_root.join("ob")).is_err());
    assert!(source.join("SKILL.md").is_file());
    assert!(fixture.service().snapshot().installed.is_empty());
}

#[cfg(unix)]
#[test]
fn copying_a_symlinked_local_source_uses_its_real_target() {
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    let source = fixture.home.join("vault/ob");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: ob\ndescription: Copy fixture\n---\n",
    )
    .unwrap();
    fs::write(source.join("example.txt"), "source data").unwrap();
    let link = fixture.home.join("ob-link");
    symlink(&source, &link).unwrap();
    fixture
        .service()
        .execute(
            SkillsCommand::ImportLocal {
                path: link.to_string_lossy().into_owned(),
                apps: vec![],
                link: false,
            },
            1,
        )
        .unwrap();
    let snapshot = fixture.service().snapshot();
    assert!(!snapshot.installed[0].linked);
    assert_eq!(file_content(&fixture.master("ob")), "source data");
}

#[test]
fn stale_staging_directories_are_cleaned_on_open_without_touching_unknown_entries() {
    let mut fixture = Fixture::new();
    let temporary = fixture.root.join("temporary");
    let checkout = temporary.join("checkout-stale");
    let skill = temporary.join("skill-stale");
    fs::create_dir_all(&checkout).unwrap();
    fs::write(checkout.join("partial"), "interrupted clone").unwrap();
    fs::create_dir_all(&skill).unwrap();
    fs::write(temporary.join("keep.txt"), "not ours").unwrap();
    fixture.reopen();
    assert!(!checkout.exists());
    assert!(!skill.exists());
    assert_eq!(
        fs::read_to_string(temporary.join("keep.txt")).unwrap(),
        "not ours"
    );
}

#[cfg(windows)]
#[test]
fn snapshot_display_paths_drop_the_verbatim_prefix() {
    let mut fixture = Fixture::new();
    let snapshot = fixture.service().snapshot();
    assert!(!snapshot.storage_path.starts_with(r"\\?\"));
    assert!(
        snapshot
            .app_paths
            .iter()
            .all(|path| !path.path.starts_with(r"\\?\"))
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
        // Valid relative path with five 100-character components (~504 bytes).
        let source_path = vec!["x".repeat(100); 5].join("/");
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
                linked: false,
                target: None,
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
            linked: false,
            target: None,
        });
    }
    let snapshot = service.snapshot();
    assert!(snapshot.truncated);
    assert!(serde_json::to_vec(&snapshot).unwrap().len() <= 700 * 1024);

    // The same catalog at the documented metadata limits can exceed 1 MiB in
    // state.json; saving must remain possible with the 2 MiB cap.
    let state = service.state.clone();
    let bytes = state_bytes(&state).unwrap();
    assert!(
        bytes.len() as u64 > 1024 * 1024,
        "state was {} bytes",
        bytes.len()
    );
    assert!(bytes.len() as u64 <= MAX_STATE_BYTES);
    service.save(state).unwrap();
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
fn unreadable_skill_metadata_is_reported_without_hiding_the_catalog() {
    let mut fixture = Fixture::new();
    fixture.commit("valid", "package");
    let broken = fixture.origin.join("skills/broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("SKILL.md"), "").unwrap();
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
            "broken metadata",
        ],
    );
    let message = fixture
        .service()
        .execute(SkillsCommand::Discover, 1)
        .unwrap();
    assert!(message.contains("skipped 1"), "{message}");
    let snapshot = fixture.service().snapshot();
    assert_eq!(snapshot.available.len(), 1);
    assert_eq!(snapshot.available[0].directory, "valid");
    assert!(
        snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("broken")),
        "{:?}",
        snapshot.warnings
    );
    // The valid entries stay visible and installable through the same catalog.
    let id = snapshot.available[0].id.clone();
    fixture
        .service()
        .execute(
            SkillsCommand::Install {
                candidate_id: id,
                apps: vec![],
            },
            2,
        )
        .unwrap();
    assert_eq!(file_content(&fixture.master("valid")), "package");

    // Once the source is fixed, the next scan drops the stale skip report.
    fs::write(broken.join("SKILL.md"), "---\nname: broken\n---\n").unwrap();
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
            "fixed metadata",
        ],
    );
    fixture
        .service()
        .execute(SkillsCommand::Discover, 3)
        .unwrap();
    let snapshot = fixture.service().snapshot();
    assert_eq!(snapshot.available.len(), 2);
    assert!(
        !snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("skipped")),
        "{:?}",
        snapshot.warnings
    );
}

#[test]
fn removing_a_source_drops_its_skip_report() {
    let mut fixture = Fixture::new();
    let broken = fixture.origin.join("skills/broken");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("SKILL.md"), "").unwrap();
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
            "broken metadata",
        ],
    );
    fixture
        .service()
        .execute(SkillsCommand::Discover, 1)
        .unwrap();
    let skipped = |fixture: &mut Fixture| {
        fixture
            .service()
            .snapshot()
            .warnings
            .iter()
            .any(|warning| warning.contains("skipped"))
    };
    assert!(skipped(&mut fixture));
    fixture
        .service()
        .execute(
            SkillsCommand::RemoveRepository {
                id: "repo-2".into(),
            },
            2,
        )
        .unwrap();
    assert!(!skipped(&mut fixture), "a removed source leaves no notice");
}

#[test]
fn a_skill_at_the_repository_root_installs_and_updates() {
    let mut fixture = Fixture::new();
    fs::write(
        fixture.origin.join("SKILL.md"),
        "---\nname: Root Skill\ndescription: Repository root package\n---\nRoot instructions\n",
    )
    .unwrap();
    fs::write(fixture.origin.join("example.txt"), "root v1").unwrap();
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
            "root skill",
        ],
    );
    fixture
        .service()
        .execute(SkillsCommand::Discover, 1)
        .unwrap();
    let snapshot = fixture.service().snapshot();
    assert_eq!(snapshot.available.len(), 1);
    assert_eq!(snapshot.available[0].path, "");
    let id = snapshot.available[0].id.clone();
    fixture
        .service()
        .execute(
            SkillsCommand::Install {
                candidate_id: id,
                apps: vec![],
            },
            2,
        )
        .unwrap();
    assert_eq!(file_content(&fixture.master("skills")), "root v1");

    fs::write(fixture.origin.join("example.txt"), "root v2").unwrap();
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
            "root update",
        ],
    );
    fixture
        .service()
        .execute(SkillsCommand::Update { skill_id: None }, 3)
        .unwrap();
    assert_eq!(file_content(&fixture.master("skills")), "root v2");
}

#[test]
fn discovery_releases_the_bare_repository_and_archive() {
    let mut fixture = Fixture::new();
    fixture.commit("review", "old");
    fixture
        .service()
        .execute(SkillsCommand::Discover, 1)
        .unwrap();
    let checkout = fixture.service().caches.get("repo-2").unwrap();
    assert!(!checkout._temporary.path().join("repository.git").exists());
    assert!(!checkout._temporary.path().join("source.tar").exists());
    assert!(checkout.tree.join("skills/review/SKILL.md").is_file());
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
