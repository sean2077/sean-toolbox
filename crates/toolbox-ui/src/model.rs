//! UI-owned input handling. Parsing is deliberately separate from execution.
use crate::i18n::UiText;
use toolbox_core::{
    AppProfile, Config, LaunchCommand, Platform, QuakeAlignment, QuakeEdge, QuakeMonitor,
    ThemePreference, UiPage,
};
use toolbox_i18n::{Locale, messages};

pub const MAX_IMPORT_BYTES: usize = 1024 * 1024;

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
                profiles: config.profiles.clone(),
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
            next.profiles.clone_from(&self.profiles);
        } else {
            for imported in &self.profiles {
                if let Some(existing) = next
                    .profiles
                    .iter_mut()
                    .find(|p| p.id.eq_ignore_ascii_case(&imported.id))
                {
                    *existing = imported.clone();
                } else {
                    next.profiles.push(imported.clone());
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
            let theme = match config.theme {
                ThemePreference::System => messages::theme_system(),
                ThemePreference::Light => messages::theme_light(),
                ThemePreference::Dark => messages::theme_dark(),
            };
            out.push_str(
                &messages::import_settings(
                    boolean(config.autostart),
                    config.ui_idle_seconds,
                    &config.language,
                    boolean(config.projects_enabled),
                    &config.launcher_hotkey,
                    theme.render(locale),
                    boolean(config.windows_enabled),
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

    #[test]
    fn new_profile_ids_respect_case_insensitive_imported_ids() {
        let mut profiles = Config::default().profiles;
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
        let mut profile = current.profiles[0].clone();
        profile.id = profile.id.to_ascii_uppercase();
        profile.name = "Changed terminal".into();
        let imported = ImportedConfig {
            profiles: vec![profile],
            config: None,
            warnings: vec![],
            filename: "test.ini".into(),
        };
        let merged = imported.apply(&current, false).unwrap();
        assert_eq!(merged.profiles.len(), current.profiles.len());
        assert_eq!(merged.profiles[0].name, "Changed terminal");
        assert_eq!(merged.launcher_hotkey, current.launcher_hotkey);
        let replaced = imported.apply(&current, true).unwrap();
        assert_eq!(replaced.profiles.len(), 1);
        assert_eq!(replaced.ui_idle_seconds, current.ui_idle_seconds);
    }

    #[test]
    fn a_merge_cannot_activate_a_duplicate_shortcut() {
        let current = Config::default();
        let mut profile = current.profiles[0].clone();
        profile.id = "another-app".into();
        profile.hotkey = current.launcher_hotkey.clone();
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
            language: "en".into(),
            autostart: true,
            ui_idle_seconds: 0,
            ..Config::default()
        };
        incoming.profiles.truncate(1);
        incoming.profiles[0].enabled = false;
        let imported = ImportedConfig::parse(
            &serde_json::to_string(&incoming).unwrap(),
            "backup.json".into(),
        )
        .unwrap();
        let merged = imported.apply(&current, false).unwrap();
        assert_eq!(merged.language, current.language);
        assert_eq!(merged.autostart, current.autostart);
        assert_eq!(merged.profiles.len(), current.profiles.len());
        assert!(!merged.profiles[0].enabled);
        assert_eq!(imported.apply(&current, true).unwrap(), incoming);
        assert!(imported.summary(Locale::En).contains("Disabled"));
    }

    #[test]
    fn import_summary_keeps_named_fields_in_their_translated_positions() {
        let mut profile = Config::default().profiles.remove(0);
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
                language: "en".into(),
                theme: ThemePreference::Dark,
                launcher_hotkey: "Ctrl+Alt+S".into(),
                autostart: true,
                projects_enabled: false,
                windows_enabled: true,
                ui_idle_seconds: 120,
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
