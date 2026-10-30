use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Windows,
    Macos,
    Linux,
}
impl Platform {
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::Macos
        } else {
            Self::Linux
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuakeEdge {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuakeAlignment {
    Start,
    #[default]
    Center,
    End,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuakeMonitor {
    #[default]
    Cursor,
    Foreground,
    Primary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct QuakeConfig {
    pub enabled: bool,
    pub edge: QuakeEdge,
    pub alignment: QuakeAlignment,
    pub width_percent: u8,
    pub height_percent: u8,
    pub monitor: QuakeMonitor,
    pub always_on_top: bool,
    pub hide_on_focus_loss: bool,
}

impl Default for QuakeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            edge: QuakeEdge::Top,
            alignment: QuakeAlignment::Center,
            width_percent: 100,
            height_percent: 50,
            monitor: QuakeMonitor::Cursor,
            always_on_top: false,
            hide_on_focus_loss: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchCommand {
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppProfile {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub matcher: String,
    pub hotkey: String,
    pub enabled: bool,
    pub maximize: bool,
    pub cycle_continuous: bool,
    #[serde(default)]
    pub quake: QuakeConfig,
    pub launch: Vec<LaunchCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub language: String,
    pub theme: ThemePreference,
    pub launcher_hotkey: String,
    pub autostart: bool,
    pub projects_enabled: bool,
    pub windows_enabled: bool,
    pub ui_idle_seconds: u64,
    pub vscode_path_override: Option<String>,
    pub profiles: Vec<AppProfile>,
}

impl Default for Config {
    fn default() -> Self {
        let platform = Platform::current();
        let (terminal, terminal_exe, code, chrome) = match platform {
            Platform::Windows => (
                "wezterm-gui.exe",
                "wezterm-gui.exe",
                "Code.exe",
                "chrome.exe",
            ),
            Platform::Macos => (
                "com.github.wez.wezterm",
                "/Applications/WezTerm.app",
                "/Applications/Visual Studio Code.app",
                "/Applications/Google Chrome.app",
            ),
            Platform::Linux => ("org.wezfurlong.wezterm", "wezterm", "code", "google-chrome"),
        };
        let make = |id: &str, name: &str, matcher: &str, exe: &str, hotkey: &str| AppProfile {
            id: id.into(),
            name: name.into(),
            platform,
            matcher: matcher.into(),
            hotkey: hotkey.into(),
            enabled: true,
            maximize: false,
            cycle_continuous: true,
            quake: QuakeConfig::default(),
            launch: vec![LaunchCommand {
                executable: exe.into(),
                args: vec![],
                cwd: None,
            }],
        };
        Self {
            schema_version: 1,
            language: "zh-CN".into(),
            theme: ThemePreference::System,
            launcher_hotkey: "Ctrl+Alt+Space".into(),
            autostart: false,
            projects_enabled: true,
            windows_enabled: true,
            ui_idle_seconds: 0,
            vscode_path_override: None,
            profiles: vec![
                make("terminal", "WezTerm", terminal, terminal_exe, "F3"),
                make(
                    "vscode",
                    "VS Code",
                    if platform == Platform::Macos {
                        "com.microsoft.VSCode"
                    } else {
                        code
                    },
                    code,
                    "F4",
                ),
                make(
                    "chrome",
                    "Chrome",
                    if platform == Platform::Macos {
                        "com.google.Chrome"
                    } else {
                        chrome
                    },
                    chrome,
                    "F5",
                ),
            ],
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("Unsupported configuration version / 不支持的配置版本".into());
        }
        if !matches!(self.language.as_str(), "zh-CN" | "en") {
            return Err("Invalid language".into());
        }
        if self.profiles.len() > 100 {
            return Err("At most 100 application profiles are supported".into());
        }
        if self.ui_idle_seconds > 3600 {
            return Err("UI idle timeout must be between 0 and 3600 seconds".into());
        }
        let launcher = HotkeySpec::parse(&self.launcher_hotkey)?;
        let mut hotkeys = HashSet::new();
        hotkeys.insert(launcher.canonical());
        let mut ids = HashSet::new();
        for p in &self.profiles {
            if !(10..=100).contains(&p.quake.width_percent)
                || !(10..=100).contains(&p.quake.height_percent)
            {
                return Err(format!(
                    "{}: Quake dimensions must be between 10 and 100 percent / 下拉窗口宽高须为 10–100%",
                    p.id
                ));
            }
            if p.id.is_empty()
                || p.id.len() > 64
                || !p
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || !ids.insert(p.id.to_ascii_lowercase())
            {
                return Err(format!("Invalid or duplicate profile id: {}", p.id));
            }
            if p.name.trim().is_empty()
                || p.name.len() > 160
                || p.matcher.trim().is_empty()
                || p.matcher.len() > 1024
                || p.matcher.contains('\0')
            {
                return Err(format!("Invalid name or matcher: {}", p.id));
            }
            let key = if p.hotkey.is_empty() {
                None
            } else {
                Some(HotkeySpec::parse(&p.hotkey)?.canonical())
            };
            if self.windows_enabled
                && p.enabled
                && p.platform == Platform::current()
                && key.is_some_and(|k| !hotkeys.insert(k))
            {
                return Err(format!("Duplicate shortcut: {}", p.hotkey));
            }
            if p.launch.is_empty() || p.launch.len() > 8 {
                return Err("Each profile needs 1–8 launch candidates".into());
            }
            for command in &p.launch {
                if command.executable.trim().is_empty()
                    || command.executable.len() > 4096
                    || command.args.len() > 128
                    || command.executable.contains('\0')
                    || command
                        .args
                        .iter()
                        .any(|a| a.contains('\0') || a.len() > 32768)
                    || command
                        .cwd
                        .as_ref()
                        .is_some_and(|s| s.contains('\0') || s.len() > 4096)
                {
                    return Err(format!("Invalid launch command: {}", p.id));
                }
            }
        }
        if self
            .vscode_path_override
            .as_ref()
            .is_some_and(|s| s.contains('\0') || s.len() > 4096)
        {
            return Err("Invalid VS Code override".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeySpec {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub super_key: bool,
    pub key: String,
}
impl HotkeySpec {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut out = Self {
            ctrl: false,
            alt: false,
            shift: false,
            super_key: false,
            key: String::new(),
        };
        for token in value.split('+').map(str::trim) {
            if token.is_empty() {
                return Err("Shortcut contains an empty key or modifier".into());
            }
            match token.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => {
                    if out.ctrl {
                        return Err("Duplicate modifier".into());
                    }
                    out.ctrl = true;
                }
                "alt" | "option" => {
                    if out.alt {
                        return Err("Duplicate modifier".into());
                    }
                    out.alt = true;
                }
                "shift" => {
                    if out.shift {
                        return Err("Duplicate modifier".into());
                    }
                    out.shift = true;
                }
                "super" | "win" | "meta" | "cmd" | "command" => {
                    if out.super_key {
                        return Err("Duplicate modifier".into());
                    }
                    out.super_key = true;
                }
                _ => {
                    if !out.key.is_empty() {
                        return Err("Shortcut must contain exactly one key".into());
                    }
                    out.key = token.to_ascii_uppercase();
                }
            }
        }
        let function_number = out
            .key
            .strip_prefix('F')
            .and_then(|x| x.parse::<u8>().ok())
            .filter(|n| (1..=24).contains(n));
        if let Some(number) = function_number {
            out.key = format!("F{number}");
        }
        let function = function_number.is_some();
        let ordinary = out.key.len() == 1 && out.key.bytes().all(|b| b.is_ascii_alphanumeric())
            || matches!(out.key.as_str(), "SPACE" | "TAB" | "ENTER" | "ESCAPE");
        if !(function || ordinary)
            || (!function && !(out.ctrl || out.alt || out.shift || out.super_key))
        {
            return Err(format!(
                "Invalid shortcut: {value}. Use modifiers + A–Z/0–9/Space/Tab/Enter/Escape, or F1–F24."
            ));
        }
        Ok(out)
    }
    pub fn canonical(&self) -> String {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.super_key {
            parts.push("Super");
        }
        parts.push(&self.key);
        parts.join("+")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcut_aliases_normalize_and_ambiguous_keys_are_rejected() {
        assert_eq!(
            HotkeySpec::parse("command + control + f01")
                .unwrap()
                .canonical(),
            "Ctrl+Super+F1"
        );
        for key in [
            "",
            "Ctrl++F3",
            "+F3",
            "F3+",
            "Ctrl+Control+K",
            "A",
            "F25",
            "Ctrl+A+B",
        ] {
            assert!(HotkeySpec::parse(key).is_err(), "{key}");
        }
    }
    #[test]
    fn active_shortcut_conflicts_and_duplicate_ids_are_rejected() {
        let mut config = Config::default();
        config.profiles[0].hotkey = config.launcher_hotkey.clone();
        assert!(config.validate().is_err());
        config.profiles[0].enabled = false;
        assert!(config.validate().is_ok());
        config.profiles[1].id = config.profiles[0].id.to_uppercase();
        assert!(config.validate().is_err());
    }

    #[test]
    fn legacy_configuration_keeps_normal_windows_and_uses_system_theme() {
        let mut value = serde_json::to_value(Config::default()).unwrap();
        value.as_object_mut().unwrap().remove("theme");
        for profile in value["profiles"].as_array_mut().unwrap() {
            profile.as_object_mut().unwrap().remove("quake");
        }
        let config: Config = serde_json::from_value(value).unwrap();
        config.validate().unwrap();
        assert_eq!(config.theme, ThemePreference::System);
        assert!(
            config
                .profiles
                .iter()
                .all(|p| p.quake == QuakeConfig::default())
        );
        let partial: QuakeConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(partial.enabled);
        assert_eq!(partial.edge, QuakeEdge::Top);
        assert_eq!(partial.alignment, QuakeAlignment::Center);
        assert_eq!(partial.monitor, QuakeMonitor::Cursor);
        assert_eq!((partial.width_percent, partial.height_percent), (100, 50));
        assert!(!partial.always_on_top);
        assert!(!partial.hide_on_focus_loss);
    }

    #[test]
    fn theme_and_quake_preferences_round_trip_and_validate_strictly() {
        let mut config = Config {
            theme: ThemePreference::Dark,
            ..Default::default()
        };
        config.profiles[0].quake = QuakeConfig {
            enabled: true,
            edge: QuakeEdge::Right,
            alignment: QuakeAlignment::End,
            width_percent: 35,
            height_percent: 80,
            monitor: QuakeMonitor::Foreground,
            always_on_top: true,
            hide_on_focus_loss: true,
        };
        config.validate().unwrap();
        let encoded = serde_json::to_string(&config).unwrap();
        assert!(encoded.contains(r#""theme":"dark""#));
        assert_eq!(serde_json::from_str::<Config>(&encoded).unwrap(), config);
        for value in [0, 9, 101, 255] {
            let mut bad = config.clone();
            bad.profiles[0].quake.width_percent = value;
            assert!(bad.validate().is_err());
            bad.profiles[0].quake.width_percent = 50;
            bad.profiles[0].quake.height_percent = value;
            assert!(bad.validate().is_err());
        }
        assert!(serde_json::from_str::<QuakeConfig>(r#"{"edge":"diagonal"}"#).is_err());
        assert!(serde_json::from_str::<QuakeConfig>(r#"{"heigth_percent":50}"#).is_err());
        assert!(serde_json::from_str::<Config>(r#"{"theme":"auto"}"#).is_err());
    }
}
