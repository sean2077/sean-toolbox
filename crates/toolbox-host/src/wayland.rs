//! KWin-backed shortcuts and window switching for a Wayland session.
//!
//! A native Wayland client cannot register global shortcuts or enumerate and
//! activate other applications' windows; only the compositor can. This module
//! renders a KWin script from the current configuration, loads it through
//! KWin's scripting D-Bus interface, and asks KGlobalAccel to replay a toggle
//! for CLI and tray requests. The script starts applications through user
//! systemd template units, so it never needs a listening host service.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
use toolbox_core::{Config, Platform};

const SCRIPT_NAME: &str = "sean-toolbox-wayland";
const SCRIPT: &str = include_str!("wayland/script.js");
const LAUNCH_UNIT: &str = "sean-toolbox-launch@.service";
const SHOW_UNIT: &str = "sean-toolbox-show@.service";

pub struct KWinHotkeys {
    script_path: PathBuf,
    unit_dir: PathBuf,
    executable: PathBuf,
    loaded: bool,
}

impl KWinHotkeys {
    /// Detect a KDE Wayland session whose compositor exposes the scripting
    /// interface. Other Wayland desktops keep the previous unsupported path.
    pub fn detect(executable: &Path) -> Result<Self, String> {
        if !is_wayland() {
            return Err("Not a Wayland session".into());
        }
        if !env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .any(|part| part.eq_ignore_ascii_case("KDE"))
        {
            return Err(
                "Wayland 全局快捷键需要 KDE Plasma 的 KWin 脚本后端 / The KWin backend requires KDE Plasma"
                    .into(),
            );
        }
        let qdbus = qdbus_command()?;
        query(
            qdbus,
            &[
                "org.kde.KWin",
                "/Scripting",
                "org.freedesktop.DBus.Peer.Ping",
            ],
        )
        .map_err(|error| format!("KWin scripting is not reachable: {error}"))?;
        let data_dir = data_dir()?.join("sean-toolbox").join("kwin");
        fs::create_dir_all(&data_dir)
            .map_err(|error| format!("Cannot create {}: {error}", data_dir.display()))?;
        let unit_dir = config_dir()?.join("systemd").join("user");
        fs::create_dir_all(&unit_dir)
            .map_err(|error| format!("Cannot create {}: {error}", unit_dir.display()))?;
        Ok(Self {
            script_path: data_dir.join("sean-toolbox-wayland.js"),
            unit_dir,
            executable: executable.to_owned(),
            loaded: false,
        })
    }

    pub fn available(&self) -> bool {
        self.loaded
    }

    /// Write the generated script and start units, then (re)load the KWin
    /// script. Configuration changes replace the script atomically.
    pub fn sync(&mut self, config: &Config) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Err(error) = self.write_units() {
            warnings.push(format!("Wayland: {error}"));
        }
        let script = render(config);
        match fs::read_to_string(&self.script_path) {
            Ok(current) if current == script && self.loaded => return warnings,
            Ok(_) | Err(_) => {}
        }
        if let Err(error) = self.write_script(&script) {
            warnings.push(format!("Wayland: {error}"));
            return warnings;
        }
        if let Err(error) = self.reload() {
            warnings.push(format!("Wayland: {error}"));
        }
        warnings
    }

    pub fn sync_script(&mut self, script: &str) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Err(error) = self.write_script(script) {
            warnings.push(format!("Wayland: {error}"));
            return warnings;
        }
        if let Err(error) = self.reload() {
            warnings.push(format!("Wayland: {error}"));
        }
        warnings
    }

    /// Replay a profile shortcut through KGlobalAccel. The script owns the
    /// action, so the host must not need window-enumeration privileges.
    pub fn invoke(&self, profile_id: &str) -> Result<(), String> {
        let title = action_title(profile_id);
        let qdbus = qdbus_command()?;
        let names = query(
            qdbus,
            &[
                "org.kde.kglobalaccel",
                "/component/kwin",
                "org.kde.kglobalaccel.Component.shortcutNames",
            ],
        )?;
        if !names.lines().any(|line| line.trim() == title) {
            return Err(format!("Wayland shortcut {title} is not registered"));
        }
        query(
            qdbus,
            &[
                "org.kde.kglobalaccel",
                "/component/kwin",
                "org.kde.kglobalaccel.Component.invokeShortcut",
                &title,
            ],
        )?;
        Ok(())
    }

    pub fn shutdown(&mut self) {
        if let Ok(qdbus) = qdbus_command() {
            let _ = query(
                qdbus,
                &[
                    "org.kde.KWin",
                    "/Scripting",
                    "org.kde.kwin.Scripting.unloadScript",
                    SCRIPT_NAME,
                ],
            );
        }
        self.loaded = false;
    }

    fn write_script(&self, script: &str) -> Result<(), String> {
        let parent = self
            .script_path
            .parent()
            .ok_or("The generated script needs a parent directory")?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        use std::io::Write;
        file.write_all(script.as_bytes())
            .and_then(|()| file.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        file.persist(&self.script_path).map_err(|error| {
            format!(
                "Cannot replace {}: {}",
                self.script_path.display(),
                error.error
            )
        })?;
        Ok(())
    }

    fn write_units(&self) -> Result<(), String> {
        let mut changed = false;
        for (name, description, arguments) in [
            (LAUNCH_UNIT, "Sean Toolbox launch %i", "launch %i"),
            (SHOW_UNIT, "Sean Toolbox show %i", "show %i"),
        ] {
            let content = format!(
                "[Unit]\nDescription={description}\n\n[Service]\nType=oneshot\nExecStart=\"{}\" {arguments}\n",
                systemd_escape(&self.executable)
            );
            let path = self.unit_dir.join(name);
            let current = fs::read_to_string(&path).ok();
            if current.as_deref() == Some(content.as_str()) {
                continue;
            }
            let mut file =
                tempfile::NamedTempFile::new_in(&self.unit_dir).map_err(|e| e.to_string())?;
            use std::io::Write;
            file.write_all(content.as_bytes())
                .and_then(|()| file.as_file().sync_all())
                .map_err(|e| e.to_string())?;
            file.persist(&path)
                .map_err(|error| format!("Cannot replace {}: {}", path.display(), error.error))?;
            changed = true;
        }
        if changed {
            let status = Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status()
                .map_err(|e| format!("Cannot run systemctl: {e}"))?;
            if !status.success() {
                return Err("systemctl --user daemon-reload failed".into());
            }
        }
        Ok(())
    }

    fn reload(&mut self) -> Result<(), String> {
        let qdbus = qdbus_command()?;
        let path = self
            .script_path
            .to_str()
            .ok_or("The script path is not valid UTF-8")?;
        let _ = query(
            qdbus,
            &[
                "org.kde.KWin",
                "/Scripting",
                "org.kde.kwin.Scripting.unloadScript",
                SCRIPT_NAME,
            ],
        );
        query(
            qdbus,
            &[
                "org.kde.KWin",
                "/Scripting",
                "org.kde.kwin.Scripting.loadScript",
                path,
                SCRIPT_NAME,
            ],
        )?;
        query(
            qdbus,
            &["org.kde.KWin", "/Scripting", "org.kde.kwin.Scripting.start"],
        )?;
        let loaded = query(
            qdbus,
            &[
                "org.kde.KWin",
                "/Scripting",
                "org.kde.kwin.Scripting.isScriptLoaded",
                SCRIPT_NAME,
            ],
        )?;
        self.loaded = loaded.trim() == "true";
        if self.loaded {
            Ok(())
        } else {
            Err("KWin did not report the generated script as loaded".into())
        }
    }
}

pub fn is_wayland() -> bool {
    env::var("XDG_SESSION_TYPE").is_ok_and(|value| value.eq_ignore_ascii_case("wayland"))
        || env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
}

/// Render the KWin script for the current configuration. Every enabled
/// profile on this platform gets an action; a profile without a shortcut is
/// registered with `none` so the CLI can still invoke its toggle.
pub fn render(config: &Config) -> String {
    let profiles: Vec<serde_json::Value> = if config.windows.enabled {
        config
            .windows
            .profiles
            .iter()
            .filter(|profile| profile.enabled && profile.platform == Platform::current())
            .map(|profile| {
                serde_json::json!({
                    "id": profile.id,
                    "name": profile.name,
                    "matcher": profile.matcher,
                    "hotkey": profile.hotkey,
                    "maximize": profile.maximize,
                    "cycle_continuous": profile.cycle_continuous,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let launcher = serde_json::to_string(&config.projects.hotkey).unwrap_or_else(|_| "null".into());
    let profiles = serde_json::to_string(&profiles).unwrap_or_else(|_| "[]".into());
    SCRIPT
        .replace("__PROFILES__", &profiles)
        .replace("__LAUNCHER__", &launcher)
}

fn action_title(profile_id: &str) -> String {
    format!("Sean Toolbox: {profile_id}")
}

fn qdbus_command() -> Result<&'static str, String> {
    for candidate in ["qdbus6", "qdbus"] {
        if env::var_os("PATH").is_some_and(|paths| {
            env::split_paths(&paths).any(|directory| directory.join(candidate).is_file())
        }) {
            return Ok(candidate);
        }
    }
    Err("qdbus6 is required for Wayland global shortcuts / 需要 qdbus6".into())
}

fn query(program: &str, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if error.is_empty() {
            format!("{program} exited with {}", output.status)
        } else {
            error
        })
    }
}

fn systemd_escape(executable: &Path) -> String {
    executable
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%")
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("HOME is not set".into())
}

fn data_dir() -> Result<PathBuf, String> {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(Ok)
        .unwrap_or_else(|| home_dir().map(|home| home.join(".local/share")))
}

fn config_dir() -> Result<PathBuf, String> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(Ok)
        .unwrap_or_else(|| home_dir().map(|home| home.join(".config")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_embeds_current_profiles_and_keeps_shortcutless_actions() {
        let mut config = Config::default();
        config.windows.profiles[0].hotkey = "F4".into();
        config.windows.profiles.push(toolbox_core::AppProfile {
            id: "code".into(),
            name: "VS Code".into(),
            platform: Platform::current(),
            matcher: "code".into(),
            hotkey: String::new(),
            enabled: true,
            maximize: false,
            cycle_continuous: true,
            quake: Default::default(),
            launch: vec![toolbox_core::LaunchCommand {
                executable: "code".into(),
                args: Vec::new(),
                cwd: None,
            }],
        });
        let script = render(&config);
        assert!(script.contains("\"id\":\"terminal\""));
        assert!(script.contains("\"hotkey\":\"F4\""));
        assert!(script.contains("\"id\":\"code\""));
        assert!(script.contains("\"hotkey\":\"\""));
        assert!(script.contains(&config.projects.hotkey));
    }

    #[test]
    fn action_titles_are_stable_for_invoke() {
        assert_eq!(action_title("vscode"), "Sean Toolbox: vscode");
    }

    #[test]
    fn systemd_executable_is_quoted_safely() {
        assert_eq!(
            systemd_escape(Path::new("/opt/Sean 100%/sean-toolbox")),
            "/opt/Sean 100%%/sean-toolbox"
        );
    }
}
