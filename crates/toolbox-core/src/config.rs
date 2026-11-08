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

impl LaunchCommand {
    pub fn is_wezterm(&self) -> bool {
        // Profiles can be edited on another platform, so accept both path separators.
        self.executable
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| {
                matches!(
                    name.to_ascii_lowercase().as_str(),
                    "wezterm" | "wezterm.exe" | "wezterm-gui" | "wezterm-gui.exe"
                )
            })
    }
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

impl AppProfile {
    pub fn is_windows_wezterm(&self) -> bool {
        self.platform == Platform::Windows
            && !self.launch.is_empty()
            && self.launch.iter().all(LaunchCommand::is_wezterm)
    }

    pub fn wezterm(platform: Platform) -> Self {
        let (matcher, executable) = match platform {
            Platform::Windows => ("wezterm-gui.exe", "wezterm-gui.exe"),
            Platform::Macos => ("com.github.wez.wezterm", "/Applications/WezTerm.app"),
            Platform::Linux => ("org.wezfurlong.wezterm", "wezterm"),
        };
        Self {
            id: "terminal".into(),
            name: "WezTerm".into(),
            platform,
            matcher: matcher.into(),
            hotkey: "F3".into(),
            enabled: true,
            maximize: false,
            cycle_continuous: true,
            quake: QuakeConfig {
                enabled: platform == Platform::Windows,
                ..Default::default()
            },
            launch: vec![LaunchCommand {
                executable: executable.into(),
                args: vec![],
                cwd: None,
            }],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GeneralConfig {
    pub language: String,
    pub theme: ThemePreference,
    pub autostart: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            language: "zh-CN".into(),
            theme: ThemePreference::System,
            autostart: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectsConfig {
    pub enabled: bool,
    pub hotkey: String,
    pub vscode_path: Option<String>,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            hotkey: "Ctrl+Alt+Space".into(),
            vscode_path: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowsConfig {
    pub enabled: bool,
    pub profiles: Vec<AppProfile>,
}

impl Default for WindowsConfig {
    fn default() -> Self {
        let platform = Platform::current();
        let (code, chrome) = match platform {
            Platform::Windows => ("Code.exe", "chrome.exe"),
            Platform::Macos => (
                "/Applications/Visual Studio Code.app",
                "/Applications/Google Chrome.app",
            ),
            Platform::Linux => ("code", "google-chrome"),
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
            enabled: true,
            profiles: vec![
                AppProfile::wezterm(platform),
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiConfig {
    pub idle_seconds: u64,
}

/// Configuration grouped by the component that owns each setting. Version 1
/// files stored these settings as top-level fields; they are migrated on load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Config {
    pub schema_version: u32,
    pub general: GeneralConfig,
    pub projects: ProjectsConfig,
    pub windows: WindowsConfig,
    pub ui: UiConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 2,
            general: GeneralConfig::default(),
            projects: ProjectsConfig::default(),
            windows: WindowsConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

impl<'de> Deserialize<'de> for Config {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let grouped = version >= 2
            || ["general", "projects", "windows", "ui"]
                .iter()
                .any(|key| value.get(*key).is_some());
        if grouped {
            let mut config: GroupedConfig =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            if config.schema_version == 0 {
                config.schema_version = 2;
            }
            Ok(config.into())
        } else {
            let legacy: LegacyConfig =
                serde_json::from_value(value).map_err(serde::de::Error::custom)?;
            Ok(legacy.into())
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupedConfig {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    general: GeneralConfig,
    #[serde(default)]
    projects: ProjectsConfig,
    #[serde(default)]
    windows: WindowsConfig,
    #[serde(default)]
    ui: UiConfig,
}

impl From<GroupedConfig> for Config {
    fn from(config: GroupedConfig) -> Self {
        Self {
            schema_version: config.schema_version,
            general: config.general,
            projects: config.projects,
            windows: config.windows,
            ui: config.ui,
        }
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LegacyConfig {
    schema_version: u32,
    language: String,
    theme: ThemePreference,
    launcher_hotkey: String,
    autostart: bool,
    projects_enabled: bool,
    windows_enabled: bool,
    ui_idle_seconds: u64,
    vscode_path_override: Option<String>,
    profiles: Vec<AppProfile>,
}

impl Default for LegacyConfig {
    fn default() -> Self {
        let config = Config::default();
        Self {
            schema_version: 0,
            language: config.general.language,
            theme: config.general.theme,
            launcher_hotkey: config.projects.hotkey,
            autostart: config.general.autostart,
            projects_enabled: config.projects.enabled,
            windows_enabled: config.windows.enabled,
            ui_idle_seconds: config.ui.idle_seconds,
            vscode_path_override: config.projects.vscode_path,
            profiles: config.windows.profiles,
        }
    }
}

impl From<LegacyConfig> for Config {
    fn from(config: LegacyConfig) -> Self {
        let mut next = Self {
            schema_version: config.schema_version,
            general: GeneralConfig {
                language: config.language,
                theme: config.theme,
                autostart: config.autostart,
            },
            projects: ProjectsConfig {
                enabled: config.projects_enabled,
                hotkey: config.launcher_hotkey,
                vscode_path: config.vscode_path_override,
            },
            windows: WindowsConfig {
                enabled: config.windows_enabled,
                profiles: config.profiles,
            },
            ui: UiConfig {
                idle_seconds: config.ui_idle_seconds,
            },
        };
        // Version 1 files are the last flat layout; a missing version stays
        // invalid instead of being silently adopted.
        if next.schema_version == 1 {
            next.schema_version = 2;
        }
        next
    }
}

/// Why a configuration is rejected. `Display` is the stable English
/// diagnostic shared by the host, IPC and logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    UnsupportedVersion,
    InvalidLanguage,
    TooManyProfiles,
    IdleTimeout,
    /// The recent-projects shortcut does not parse; carries the parser reason.
    LauncherShortcut(String),
    /// A profile shortcut does not parse: profile id and parser reason.
    ProfileShortcut(String, String),
    QuakeDimensions(String),
    InvalidProfileId(String),
    InvalidNameOrMatcher(String),
    DuplicateShortcut(String),
    LaunchCandidates(String),
    InvalidLaunchCommand(String),
    InvalidVsCodePath,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion => {
                f.write_str("Unsupported configuration version / 不支持的配置版本")
            }
            Self::InvalidLanguage => f.write_str("Invalid language"),
            Self::TooManyProfiles => f.write_str("At most 100 application profiles are supported"),
            Self::IdleTimeout => f.write_str("UI idle timeout must be between 0 and 3600 seconds"),
            Self::LauncherShortcut(error) | Self::ProfileShortcut(_, error) => f.write_str(error),
            Self::QuakeDimensions(id) => write!(
                f,
                "{id}: Quake dimensions must be between 10 and 100 percent / 下拉窗口宽高须为 10–100%"
            ),
            Self::InvalidProfileId(id) => write!(f, "Invalid or duplicate profile id: {id}"),
            Self::InvalidNameOrMatcher(id) => write!(f, "Invalid name or matcher: {id}"),
            Self::DuplicateShortcut(hotkey) => write!(f, "Duplicate shortcut: {hotkey}"),
            Self::LaunchCandidates(_) => f.write_str("Each profile needs 1–8 launch candidates"),
            Self::InvalidLaunchCommand(id) => write!(f, "Invalid launch command: {id}"),
            Self::InvalidVsCodePath => f.write_str("Invalid VS Code override"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Validates with the stable English diagnostic used by the host and IPC.
    pub fn validate(&self) -> Result<(), String> {
        self.check().map_err(|error| error.to_string())
    }

    /// Validates and keeps the reason, so a UI can present it in its own language.
    pub fn check(&self) -> Result<(), ConfigError> {
        if self.schema_version != 2 {
            return Err(ConfigError::UnsupportedVersion);
        }
        if !matches!(self.general.language.as_str(), "zh-CN" | "en") {
            return Err(ConfigError::InvalidLanguage);
        }
        if self.windows.profiles.len() > 100 {
            return Err(ConfigError::TooManyProfiles);
        }
        if self.ui.idle_seconds > 3600 {
            return Err(ConfigError::IdleTimeout);
        }
        let launcher =
            HotkeySpec::parse(&self.projects.hotkey).map_err(ConfigError::LauncherShortcut)?;
        let mut hotkeys = HashSet::new();
        hotkeys.insert(launcher.canonical());
        let mut ids = HashSet::new();
        for p in &self.windows.profiles {
            if !(10..=100).contains(&p.quake.width_percent)
                || !(10..=100).contains(&p.quake.height_percent)
            {
                return Err(ConfigError::QuakeDimensions(p.id.clone()));
            }
            if p.id.is_empty()
                || p.id.len() > 64
                || !p
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || !ids.insert(p.id.to_ascii_lowercase())
            {
                return Err(ConfigError::InvalidProfileId(p.id.clone()));
            }
            if p.name.trim().is_empty()
                || p.name.len() > 160
                || p.matcher.trim().is_empty()
                || p.matcher.len() > 1024
                || p.matcher.contains('\0')
            {
                return Err(ConfigError::InvalidNameOrMatcher(p.id.clone()));
            }
            let key = if p.hotkey.is_empty() {
                None
            } else {
                Some(
                    HotkeySpec::parse(&p.hotkey)
                        .map_err(|error| ConfigError::ProfileShortcut(p.id.clone(), error))?
                        .canonical(),
                )
            };
            if self.windows.enabled
                && p.enabled
                && p.platform == Platform::current()
                && key.is_some_and(|k| !hotkeys.insert(k))
            {
                return Err(ConfigError::DuplicateShortcut(p.hotkey.clone()));
            }
            if p.launch.is_empty() || p.launch.len() > 8 {
                return Err(ConfigError::LaunchCandidates(p.id.clone()));
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
                    return Err(ConfigError::InvalidLaunchCommand(p.id.clone()));
                }
            }
        }
        if self
            .projects
            .vscode_path
            .as_ref()
            .is_some_and(|s| s.contains('\0') || s.len() > 4096)
        {
            return Err(ConfigError::InvalidVsCodePath);
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
    fn wezterm_dropdown_recognition_uses_program_names_not_profile_labels() {
        let mut profile = AppProfile::wezterm(Platform::Windows);
        profile.name = "My terminal".into();
        profile.matcher = "a-user-supplied-match".into();
        for executable in [
            "wezterm",
            "wezterm.exe",
            "wezterm-gui",
            "WezTerm-GUI.EXE",
            r"%ProgramFiles%\WezTerm\wezterm-gui.exe",
            "C:/Apps/WezTerm/wezterm.exe",
        ] {
            profile.launch[0].executable = executable.into();
            assert!(profile.is_windows_wezterm(), "{executable}");
        }
        for executable in [
            "",
            "my-wezterm.exe",
            "wezterm.exe.bat",
            "cmd.exe",
            "WezTerm.app",
        ] {
            profile.launch[0].executable = executable.into();
            assert!(!profile.is_windows_wezterm(), "{executable}");
        }
        profile.launch[0].executable = "wezterm-gui.exe".into();
        profile.launch.push(LaunchCommand {
            executable: "pwsh.exe".into(),
            args: vec![],
            cwd: None,
        });
        assert!(!profile.is_windows_wezterm());
        profile.launch.clear();
        assert!(!profile.is_windows_wezterm());
        assert!(!AppProfile::wezterm(Platform::Linux).is_windows_wezterm());
        assert!(!AppProfile::wezterm(Platform::Macos).is_windows_wezterm());
    }

    #[test]
    fn wezterm_preset_enables_a_windows_dropdown_without_migrating_saved_profiles() {
        let mut profile = AppProfile::wezterm(Platform::Windows);
        assert!(profile.quake.enabled);
        assert_eq!(profile.hotkey, "F3");
        assert_eq!(profile.quake.edge, QuakeEdge::Top);
        assert_eq!(
            (profile.quake.width_percent, profile.quake.height_percent),
            (100, 50)
        );
        assert!(!AppProfile::wezterm(Platform::Linux).quake.enabled);
        assert!(!AppProfile::wezterm(Platform::Macos).quake.enabled);

        profile.quake.enabled = false;
        profile.hotkey = "Ctrl+Alt+T".into();
        profile.launch[0].args = vec!["start".into(), "--cwd".into(), "D:/Work/My Project".into()];
        let saved = serde_json::to_string(&profile).unwrap();
        assert_eq!(serde_json::from_str::<AppProfile>(&saved).unwrap(), profile);
    }

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
        config.windows.profiles[0].hotkey = config.projects.hotkey.clone();
        assert!(config.validate().is_err());
        config.windows.profiles[0].enabled = false;
        assert!(config.validate().is_ok());
        config.windows.profiles[1].id = config.windows.profiles[0].id.to_uppercase();
        assert!(config.validate().is_err());
    }

    fn legacy_value() -> serde_json::Value {
        let mut value = serde_json::json!({
            "schema_version": 1,
            "language": "zh-CN",
            "theme": "system",
            "launcher_hotkey": "Ctrl+Alt+Space",
            "autostart": false,
            "projects_enabled": true,
            "windows_enabled": true,
            "ui_idle_seconds": 0,
            "vscode_path_override": null,
            "profiles": serde_json::to_value(Config::default().windows.profiles).unwrap(),
        });
        value.as_object_mut().unwrap().remove("theme");
        for profile in value["profiles"].as_array_mut().unwrap() {
            profile.as_object_mut().unwrap().remove("quake");
        }
        value
    }

    #[test]
    fn legacy_configuration_keeps_normal_windows_and_migrates_to_grouped_schema() {
        let config: Config = serde_json::from_value(legacy_value()).unwrap();
        config.validate().unwrap();
        assert_eq!(config.schema_version, 2);
        assert_eq!(config.general.theme, ThemePreference::System);
        assert!(
            config
                .windows
                .profiles
                .iter()
                .all(|p| p.quake == QuakeConfig::default())
        );
        let encoded = serde_json::to_value(&config).unwrap();
        assert_eq!(encoded["schema_version"], 2);
        assert!(encoded.get("language").is_none());
        assert_eq!(encoded["general"]["language"], "zh-CN");
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
    fn typed_validation_keeps_the_english_host_diagnostics() {
        let mut config = Config::default();
        config.ui.idle_seconds = 3601;
        assert_eq!(config.check(), Err(ConfigError::IdleTimeout));
        assert_eq!(
            config.validate().unwrap_err(),
            "UI idle timeout must be between 0 and 3600 seconds"
        );
        let mut config = Config::default();
        config.windows.profiles[0].id = "bad id".into();
        assert_eq!(
            config.check(),
            Err(ConfigError::InvalidProfileId("bad id".into()))
        );
        assert_eq!(
            config.validate().unwrap_err(),
            "Invalid or duplicate profile id: bad id"
        );
        config.windows.profiles[0].id = "app".into();
        config.projects.vscode_path = Some("bad\0path".into());
        assert_eq!(config.check(), Err(ConfigError::InvalidVsCodePath));
        assert_eq!(config.validate().unwrap_err(), "Invalid VS Code override");
    }

    #[test]
    fn grouped_configuration_fills_missing_components() {
        let config: Config =
            serde_json::from_str(r#"{"schema_version":2,"general":{"language":"en"}}"#).unwrap();
        config.validate().unwrap();
        assert_eq!(config.general.language, "en");
        assert_eq!(config.general.theme, ThemePreference::System);
        assert!(config.projects.enabled);
        assert_eq!(config.projects.hotkey, "Ctrl+Alt+Space");
        assert!(config.windows.enabled);
        assert!(!config.windows.profiles.is_empty());
        assert_eq!(config.ui.idle_seconds, 0);
    }

    #[test]
    fn missing_schema_version_and_unknown_fields_are_still_rejected() {
        let mut missing = legacy_value();
        missing.as_object_mut().unwrap().remove("schema_version");
        let missing: Config = serde_json::from_value(missing).unwrap();
        assert!(missing.validate().is_err());

        let mut unknown = serde_json::to_value(Config::default()).unwrap();
        unknown["general"]["langauge"] = "en".into();
        assert!(serde_json::from_value::<Config>(unknown).is_err());
    }

    #[test]
    fn theme_and_quake_preferences_round_trip_and_validate_strictly() {
        let mut config = Config {
            general: GeneralConfig {
                theme: ThemePreference::Dark,
                ..Default::default()
            },
            ..Default::default()
        };
        config.windows.profiles[0].quake = QuakeConfig {
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
            bad.windows.profiles[0].quake.width_percent = value;
            assert!(bad.validate().is_err());
            bad.windows.profiles[0].quake.width_percent = 50;
            bad.windows.profiles[0].quake.height_percent = value;
            assert!(bad.validate().is_err());
        }
        assert!(serde_json::from_str::<QuakeConfig>(r#"{"edge":"diagonal"}"#).is_err());
        assert!(serde_json::from_str::<QuakeConfig>(r#"{"heigth_percent":50}"#).is_err());
        assert!(serde_json::from_str::<Config>(r#"{"theme":"auto"}"#).is_err());
    }
}
