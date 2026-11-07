//! UI-owned input handling. Parsing is deliberately separate from execution.
use crate::i18n::UiText;
use toolbox_core::{
    AppProfile, Config, HotkeySpec, LaunchCommand, Platform, QuakeAlignment, QuakeEdge,
    QuakeMonitor, ThemePreference, UiPage,
};
use toolbox_i18n::{Locale, messages};

pub const MAX_IMPORT_BYTES: usize = 1024 * 1024;

/// Shorten only the presentation path. Identity and launch targets stay untouched.
pub fn project_display_path(location: &str) -> String {
    let chars = location.chars().collect::<Vec<_>>();
    if chars.len() <= 52 {
        return location.to_owned();
    }
    let prefix = chars[..18].iter().collect::<String>();
    let suffix = chars[chars.len() - 30..].iter().collect::<String>();
    format!("{prefix}…{suffix}")
}

pub fn text_matches(query: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

/// Suggestions must never overwrite an existing name or matching rule.
/// Splitting both separators also supports editing another platform's profile.
pub fn program_suggestion(path: &str, platform: Platform) -> (String, Option<String>) {
    let file = path
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let stem = file
        .strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .or_else(|| file.strip_suffix(".app"))
        .unwrap_or(file);
    let name = match stem.to_ascii_lowercase().as_str() {
        "code" => "Visual Studio Code".to_owned(),
        "wezterm-gui" | "wezterm" => "WezTerm".to_owned(),
        "chrome" => "Google Chrome".to_owned(),
        _ => stem.to_owned(),
    };
    // A macOS bundle ID cannot be inferred safely from the bundle's filename.
    let matcher = match platform {
        Platform::Windows | Platform::Linux => Some(file.to_owned()),
        Platform::Macos => None,
    };
    (name, matcher)
}

/// Convert Slint's logical key event to the existing configuration syntax.
/// Slint swaps Control/Meta on macOS for cross-platform UI shortcuts; global
/// registrations use the physical Control/Command meaning instead.
pub fn recorded_shortcut(
    text: &str,
    control: bool,
    alt: bool,
    shift: bool,
    meta: bool,
    platform: Platform,
) -> Result<Option<String>, UiText> {
    use slint::platform::Key;
    let mut chars = text.chars();
    let Some(character) = chars.next() else {
        return Ok(None);
    };
    if chars.next().is_some() {
        return Err(messages::shortcut_supported_keys().into());
    }
    if [
        Key::Control,
        Key::ControlR,
        Key::Shift,
        Key::ShiftR,
        Key::Alt,
        Key::AltGr,
        Key::Meta,
        Key::MetaR,
    ]
    .into_iter()
    .any(|key| character == char::from(key))
    {
        return Ok(None);
    }
    let key = if character.is_ascii_alphanumeric() {
        character.to_ascii_uppercase().to_string()
    } else if character == char::from(Key::Space) {
        "SPACE".to_owned()
    } else if character == char::from(Key::Tab) || character == char::from(Key::Backtab) {
        "TAB".to_owned()
    } else if character == char::from(Key::Return) {
        "ENTER".to_owned()
    } else if character == char::from(Key::Escape) {
        "ESCAPE".to_owned()
    } else if (char::from(Key::F1)..=char::from(Key::F24)).contains(&character) {
        format!("F{}", character as u32 - char::from(Key::F1) as u32 + 1)
    } else {
        return Err(messages::shortcut_supported_keys().into());
    };
    let (ctrl, super_key) = if platform == Platform::Macos {
        (meta, control)
    } else {
        (control, meta)
    };
    let key = HotkeySpec {
        ctrl,
        alt,
        shift,
        super_key,
        key,
    }
    .canonical();
    HotkeySpec::parse(&key)
        .map(|_| Some(key))
        .map_err(|_| messages::shortcut_supported_keys().into())
}

/// Validate syntax for every profile; only active, current-platform bindings
/// can conflict with the host's registrations. Never probe other applications.
pub fn shortcut_problem(
    value: &str,
    config: &Config,
    editing: Option<usize>,
    active: bool,
    launcher: bool,
) -> Option<toolbox_i18n::Message> {
    if value.trim().is_empty() && !launcher {
        return None;
    }
    let Ok(key) = HotkeySpec::parse(value) else {
        return Some(messages::shortcut_supported_keys());
    };
    if !active {
        return None;
    }
    let canonical = key.canonical();
    if !launcher
        && HotkeySpec::parse(&config.projects.hotkey)
            .is_ok_and(|other| other.canonical() == canonical)
    {
        return Some(messages::shortcut_conflict_launcher());
    }
    if config.windows.enabled {
        for (index, profile) in config.windows.profiles.iter().enumerate() {
            if Some(index) != editing
                && profile.enabled
                && profile.platform == Platform::current()
                && HotkeySpec::parse(&profile.hotkey)
                    .is_ok_and(|other| other.canonical() == canonical)
            {
                return Some(messages::shortcut_conflict_app(&profile.name));
            }
        }
    }
    None
}

#[derive(Clone, Debug)]
pub struct ImportedConfig {
    pub profiles: Vec<AppProfile>,
    pub config: Option<Config>,
    pub warnings: Vec<String>,
    pub filename: String,
}

impl ImportedConfig {
    pub fn parse(text: &str, filename: String) -> Result<Self, UiText> {
        if text.len() > MAX_IMPORT_BYTES {
            return Err(messages::config_too_large().into());
        }
        let text = text.trim_start_matches('\u{feff}');
        if filename.to_ascii_lowercase().ends_with(".json") || text.trim_start().starts_with('{') {
            let config: Config =
                serde_json::from_str(text).map_err(messages::config_invalid_json)?;
            config.validate()?;
            Ok(Self {
                profiles: config.windows.profiles.clone(),
                config: Some(config),
                warnings: vec![],
                filename,
            })
        } else {
            let preview = toolbox_core::import::parse_ini(text)?;
            Ok(Self {
                profiles: preview.profiles,
                config: None,
                warnings: preview.warnings,
                filename,
            })
        }
    }

    pub fn apply(&self, current: &Config, replace: bool) -> Result<Config, String> {
        let mut next = if replace {
            self.config.clone().unwrap_or_else(|| current.clone())
        } else {
            current.clone()
        };
        if replace {
            next.windows.profiles.clone_from(&self.profiles);
        } else {
            for imported in &self.profiles {
                if let Some(existing) = next
                    .windows
                    .profiles
                    .iter_mut()
                    .find(|p| p.id.eq_ignore_ascii_case(&imported.id))
                {
                    *existing = imported.clone();
                } else {
                    next.windows.profiles.push(imported.clone());
                }
            }
        }
        next.validate()?;
        Ok(next)
    }

    pub fn summary(&self, locale: Locale) -> String {
        let boolean = |value| {
            if value {
                messages::yes()
            } else {
                messages::no()
            }
            .render(locale)
        };
        let mut out = messages::import_heading(self.profiles.len(), &self.filename).render(locale);
        for profile in &self.profiles {
            let enabled = if profile.enabled {
                messages::enabled()
            } else {
                messages::disabled()
            };
            out.push_str(
                &messages::import_profile(
                    enabled.render(locale),
                    &profile.id,
                    &profile.matcher,
                    &profile.name,
                    platform_name(profile.platform),
                    &profile.hotkey,
                )
                .render(locale),
            );
            if profile.quake.enabled {
                let q = &profile.quake;
                let edge = match q.edge {
                    QuakeEdge::Top => messages::edge_top(),
                    QuakeEdge::Bottom => messages::edge_bottom(),
                    QuakeEdge::Left => messages::edge_left(),
                    QuakeEdge::Right => messages::edge_right(),
                };
                let monitor = match q.monitor {
                    QuakeMonitor::Cursor => messages::monitor_cursor(),
                    QuakeMonitor::Foreground => messages::monitor_foreground(),
                    QuakeMonitor::Primary => messages::monitor_primary(),
                };
                out.push_str(
                    &messages::import_quake(
                        boolean(q.hide_on_focus_loss),
                        edge.render(locale),
                        q.height_percent,
                        monitor.render(locale),
                        boolean(q.always_on_top),
                        q.width_percent,
                    )
                    .render(locale),
                );
            }
            for (index, command) in profile.launch.iter().enumerate() {
                out.push_str(&format!(
                    "  {}. {} {}\n",
                    index + 1,
                    command.executable,
                    serde_json::to_string(&command.args).unwrap_or_default(),
                ));
                if let Some(cwd) = &command.cwd {
                    out.push_str(&messages::import_working_directory(cwd).render(locale));
                }
            }
        }
        if let Some(config) = &self.config {
            let theme = match config.general.theme {
                ThemePreference::System => messages::theme_system(),
                ThemePreference::Light => messages::theme_light(),
                ThemePreference::Dark => messages::theme_dark(),
            };
            out.push_str(
                &messages::import_settings(
                    boolean(config.general.autostart),
                    config.ui.idle_seconds,
                    &config.general.language,
                    boolean(config.projects.enabled),
                    &config.projects.hotkey,
                    theme.render(locale),
                    boolean(config.windows.enabled),
                )
                .render(locale),
            );
        }
        if !self.warnings.is_empty() {
            out.push_str(&messages::import_notes().render(locale));
            for warning in &self.warnings {
                out.push_str(&format!("• {warning}\n"));
            }
        }
        out
    }
}

pub fn launch_command(
    executable: &str,
    arguments: &str,
    cwd: &str,
) -> Result<LaunchCommand, UiText> {
    let args: Vec<String> = serde_json::from_str(arguments).map_err(messages::invalid_arguments)?;
    Ok(LaunchCommand {
        executable: executable.trim().to_owned(),
        args,
        cwd: (!cwd.trim().is_empty()).then(|| cwd.trim().to_owned()),
    })
}

pub fn next_profile_id(profiles: &[AppProfile]) -> String {
    // Config identity is case-insensitive, including IDs imported from INI/JSON.
    let mut serial = 1;
    loop {
        let id = format!("app-{serial}");
        if !profiles
            .iter()
            .any(|profile| profile.id.eq_ignore_ascii_case(&id))
        {
            return id;
        }
        serial += 1;
    }
}

pub fn platform_name(platform: Platform) -> &'static str {
    match platform {
        Platform::Windows => "Windows",
        Platform::Macos => "macOS",
        Platform::Linux => "Linux",
    }
}
pub fn platform_index(platform: Platform) -> i32 {
    match platform {
        Platform::Windows => 0,
        Platform::Macos => 1,
        Platform::Linux => 2,
    }
}
pub fn platform_from_index(index: i32) -> Platform {
    match index {
        0 => Platform::Windows,
        1 => Platform::Macos,
        _ => Platform::Linux,
    }
}

pub fn theme_index(theme: ThemePreference) -> i32 {
    match theme {
        ThemePreference::System => 0,
        ThemePreference::Light => 1,
        ThemePreference::Dark => 2,
    }
}

pub fn theme_from_index(index: i32) -> ThemePreference {
    match index {
        1 => ThemePreference::Light,
        2 => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

pub fn quake_edge_index(edge: QuakeEdge) -> i32 {
    match edge {
        QuakeEdge::Top => 0,
        QuakeEdge::Bottom => 1,
        QuakeEdge::Left => 2,
        QuakeEdge::Right => 3,
    }
}

pub fn quake_edge_from_index(index: i32) -> QuakeEdge {
    match index {
        1 => QuakeEdge::Bottom,
        2 => QuakeEdge::Left,
        3 => QuakeEdge::Right,
        _ => QuakeEdge::Top,
    }
}

pub fn quake_alignment_index(alignment: QuakeAlignment) -> i32 {
    match alignment {
        QuakeAlignment::Start => 0,
        QuakeAlignment::Center => 1,
        QuakeAlignment::End => 2,
    }
}

pub fn quake_alignment_from_index(index: i32) -> QuakeAlignment {
    match index {
        0 => QuakeAlignment::Start,
        2 => QuakeAlignment::End,
        _ => QuakeAlignment::Center,
    }
}

pub fn quake_monitor_index(monitor: QuakeMonitor) -> i32 {
    match monitor {
        QuakeMonitor::Cursor => 0,
        QuakeMonitor::Foreground => 1,
        QuakeMonitor::Primary => 2,
    }
}

pub fn quake_monitor_from_index(index: i32) -> QuakeMonitor {
    match index {
        1 => QuakeMonitor::Foreground,
        2 => QuakeMonitor::Primary,
        _ => QuakeMonitor::Cursor,
    }
}
pub fn page_index(page: UiPage) -> i32 {
    match page {
        UiPage::Projects => 0,
        UiPage::Windows => 1,
        UiPage::Settings => 2,
        UiPage::Diagnostics => 3,
        UiPage::Skills => 4,
    }
}
pub fn page_from_index(index: i32) -> UiPage {
    match index {
        1 => UiPage::Windows,
        2 => UiPage::Settings,
        3 => UiPage::Diagnostics,
        4 => UiPage::Skills,
        _ => UiPage::Projects,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toolbox_core::{GeneralConfig, ProjectsConfig, UiConfig, WindowsConfig};

    #[test]
    fn display_paths_keep_identity_and_unicode_suffixes_readable() {
        for short in [
            r"C:\work\sean-toolbox",
            "/home/sean/项目",
            "vscode-remote://ssh-remote+dev/repo",
        ] {
            assert_eq!(project_display_path(short), short);
        }
        let long = format!("C:\\项目\\{}\\src\\main.rs", "汉".repeat(70));
        let shown = project_display_path(&long);
        assert_eq!(shown.chars().count(), 49);
        assert!(shown.starts_with("C:\\项目\\"));
        assert!(shown.ends_with("\\src\\main.rs"));
        assert!(shown.contains('…'));
        assert!(long.chars().count() > 52);
        let remote =
            "vscode-remote://ssh-remote+production/home/sean/work/very-long-project-location/src";
        assert!(project_display_path(remote).ends_with("-project-location/src"));
    }

    #[test]
    fn application_search_matches_all_words_without_case_or_whitespace_surprises() {
        assert!(text_matches(
            "  CODE  f4 ",
            "Visual Studio Code Code.exe F4"
        ));
        assert!(text_matches("工具 中文", "中文工具箱"));
        assert!(text_matches("", "WezTerm"));
        assert!(!text_matches("Code F3", "Visual Studio Code F4"));
    }

    #[test]
    fn program_suggestions_use_filenames_but_do_not_invent_bundle_ids() {
        assert_eq!(
            program_suggestion(
                r"C:\Program Files\WezTerm\wezterm-gui.exe",
                Platform::Windows
            ),
            ("WezTerm".into(), Some("wezterm-gui.exe".into()))
        );
        assert_eq!(
            program_suggestion("/usr/bin/code", Platform::Linux),
            ("Visual Studio Code".into(), Some("code".into()))
        );
        assert_eq!(
            program_suggestion("/Applications/Visual Studio Code.app", Platform::Macos),
            ("Visual Studio Code".into(), None)
        );
    }

    #[test]
    fn shortcut_recording_preserves_native_modifier_meaning() {
        use slint::platform::Key;
        assert_eq!(
            recorded_shortcut("t", true, true, false, false, Platform::Windows).unwrap(),
            Some("Ctrl+Alt+T".into())
        );
        assert_eq!(
            recorded_shortcut("k", true, false, false, false, Platform::Macos).unwrap(),
            Some("Super+K".into())
        );
        assert_eq!(
            recorded_shortcut("k", false, false, false, true, Platform::Macos).unwrap(),
            Some("Ctrl+K".into())
        );
        assert_eq!(
            recorded_shortcut(
                &char::from(Key::F24).to_string(),
                false,
                false,
                false,
                false,
                Platform::Windows
            )
            .unwrap(),
            Some("F24".into())
        );
        assert_eq!(
            recorded_shortcut(
                &char::from(Key::Control).to_string(),
                true,
                false,
                false,
                false,
                Platform::Windows
            )
            .unwrap(),
            None
        );
        assert!(recorded_shortcut("a", false, false, false, false, Platform::Windows).is_err());
        assert!(recorded_shortcut("中", true, false, false, false, Platform::Windows).is_err());
    }

    #[test]
    fn shortcut_feedback_checks_only_bindings_that_will_be_registered() {
        let config = Config::default();
        assert!(shortcut_problem("control+alt+space", &config, None, true, false).is_some());
        assert!(
            shortcut_problem(
                &config.windows.profiles[0].hotkey,
                &config,
                Some(0),
                true,
                false
            )
            .is_none()
        );
        let conflict = shortcut_problem(
            &config.windows.profiles[1].hotkey,
            &config,
            Some(0),
            true,
            false,
        )
        .unwrap();
        assert!(
            conflict
                .render(Locale::En)
                .contains(&config.windows.profiles[1].name)
        );
        assert!(
            shortcut_problem(
                &config.windows.profiles[1].hotkey,
                &config,
                Some(0),
                false,
                false
            )
            .is_none()
        );
        assert!(shortcut_problem("", &config, Some(0), true, false).is_none());
        assert!(shortcut_problem("", &config, None, true, true).is_some());
    }

    #[test]
    fn new_profile_ids_respect_case_insensitive_imported_ids() {
        let mut profiles = Config::default().windows.profiles;
        profiles[0].id = "APP-1".into();
        profiles[1].id = "app-3".into();
        assert_eq!(next_profile_id(&profiles), "app-2");
        profiles[2].id = "App-2".into();
        assert_eq!(next_profile_id(&profiles), "app-4");
        assert_eq!(next_profile_id(&[]), "app-1");
    }

    #[test]
    fn arguments_preserve_boundaries_and_literal_shell_characters() {
        let command = launch_command(
            "code",
            r#"["a path/with spaces", "$(touch nope)", "--flag=\"literal\""]"#,
            "",
        )
        .unwrap();
        assert_eq!(
            command.args,
            ["a path/with spaces", "$(touch nope)", "--flag=\"literal\""]
        );
        assert_eq!(command.cwd, None);
        assert!(launch_command("code", "--new-window", "").is_err());
        assert!(launch_command("code", "[42]", "").is_err());
    }

    #[test]
    fn merge_replaces_ids_case_insensitively_and_preserves_settings() {
        let current = Config::default();
        let mut profile = current.windows.profiles[0].clone();
        profile.id = profile.id.to_ascii_uppercase();
        profile.name = "Changed terminal".into();
        let imported = ImportedConfig {
            profiles: vec![profile],
            config: None,
            warnings: vec![],
            filename: "test.ini".into(),
        };
        let merged = imported.apply(&current, false).unwrap();
        assert_eq!(
            merged.windows.profiles.len(),
            current.windows.profiles.len()
        );
        assert_eq!(merged.windows.profiles[0].name, "Changed terminal");
        assert_eq!(merged.projects.hotkey, current.projects.hotkey);
        let replaced = imported.apply(&current, true).unwrap();
        assert_eq!(replaced.windows.profiles.len(), 1);
        assert_eq!(replaced.ui.idle_seconds, current.ui.idle_seconds);
    }

    #[test]
    fn a_merge_cannot_activate_a_duplicate_shortcut() {
        let current = Config::default();
        let mut profile = current.windows.profiles[0].clone();
        profile.id = "another-app".into();
        profile.hotkey = current.projects.hotkey.clone();
        let imported = ImportedConfig {
            profiles: vec![profile],
            config: None,
            warnings: vec![],
            filename: "test.ini".into(),
        };
        assert!(imported.apply(&current, false).is_err());
    }

    #[test]
    fn json_replacement_restores_settings_but_merge_only_changes_apps() {
        let current = Config::default();
        let mut incoming = Config {
            general: GeneralConfig {
                language: "en".into(),
                autostart: true,
                ..Default::default()
            },
            ui: UiConfig { idle_seconds: 0 },
            ..Config::default()
        };
        incoming.windows.profiles.truncate(1);
        incoming.windows.profiles[0].enabled = false;
        let imported = ImportedConfig::parse(
            &serde_json::to_string(&incoming).unwrap(),
            "backup.json".into(),
        )
        .unwrap();
        let merged = imported.apply(&current, false).unwrap();
        assert_eq!(merged.general.language, current.general.language);
        assert_eq!(merged.general.autostart, current.general.autostart);
        assert_eq!(
            merged.windows.profiles.len(),
            current.windows.profiles.len()
        );
        assert!(!merged.windows.profiles[0].enabled);
        assert_eq!(imported.apply(&current, true).unwrap(), incoming);
        assert!(imported.summary(Locale::En).contains("Disabled"));
    }

    #[test]
    fn import_summary_keeps_named_fields_in_their_translated_positions() {
        let mut profile = Config::default().windows.profiles.remove(0);
        profile.name = "Distinct {name}".into();
        profile.id = "unique-id".into();
        profile.matcher = "Matcher.exe".into();
        profile.hotkey = "Ctrl+Alt+D".into();
        profile.platform = Platform::Windows;
        profile.enabled = false;
        profile.quake.enabled = true;
        profile.quake.edge = QuakeEdge::Right;
        profile.quake.width_percent = 40;
        profile.quake.height_percent = 75;
        profile.quake.monitor = QuakeMonitor::Foreground;
        profile.quake.always_on_top = true;
        profile.quake.hide_on_focus_loss = false;
        let imported = ImportedConfig {
            profiles: vec![profile],
            config: Some(Config {
                general: GeneralConfig {
                    language: "en".into(),
                    theme: ThemePreference::Dark,
                    autostart: true,
                },
                projects: ProjectsConfig {
                    enabled: false,
                    hotkey: "Ctrl+Alt+S".into(),
                    ..Default::default()
                },
                windows: WindowsConfig {
                    enabled: true,
                    ..Default::default()
                },
                ui: UiConfig { idle_seconds: 120 },
                ..Config::default()
            }),
            warnings: Vec::new(),
            filename: "backup.json".into(),
        };
        let english = imported.summary(Locale::En);
        assert!(english.contains("1 application profile\n"));
        assert!(english.contains(
            "• Distinct {name}  [Windows]  Ctrl+Alt+D · Disabled\n  ID: unique-id\n  Matcher.exe\n"
        ));
        assert!(english.contains(
            "Quake: Right, 40% × 75%, display=With foreground window, topmost=Yes, auto-hide=No"
        ));
        assert!(english.contains("language=en, theme=Dark, shortcut=Ctrl+Alt+S, autostart=Yes, projects=No, windows=Yes, UI idle=120 s"));
        let chinese = imported.summary(Locale::ZhCn);
        assert!(chinese.contains(
            "• Distinct {name}  [Windows]  Ctrl+Alt+D · 已禁用\n  标识：unique-id\n  Matcher.exe\n"
        ));
        assert!(chinese.contains(
            "Quake 停靠：右侧，宽 40% × 高 75%，显示器=前台窗口所在，置顶=是，失焦收起=否"
        ));
        assert!(chinese.contains("语言=en，主题=深色，唤出快捷键=Ctrl+Alt+S，自启=是，项目模块=否，窗口模块=是，界面释放=120 秒"));
    }

    #[test]
    fn import_rejects_oversize_and_unknown_configuration_fields() {
        assert!(ImportedConfig::parse(&"x".repeat(MAX_IMPORT_BYTES + 1), "a.ini".into()).is_err());
        assert!(
            ImportedConfig::parse(r#"{"schema_version":1,"unexpected":"no"}"#, "a.json".into())
                .is_err()
        );
    }
}
