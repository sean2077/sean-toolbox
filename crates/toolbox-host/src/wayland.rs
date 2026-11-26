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
use toolbox_core::{Config, HotkeySpec, Platform};
use zbus::blocking::{Connection, Proxy};

const SCRIPT_NAME: &str = "sean-toolbox-wayland";
const SCRIPT: &str = include_str!("wayland/script.js");
const LAUNCH_UNIT: &str = "sean-toolbox-launch@.service";
const SHOW_UNIT: &str = "sean-toolbox-show@.service";
const KWIN_COMPONENT: &str = "kwin";

/// A shortcut the KWin script owns and the binding it expects in
/// KGlobalAccel. KWin keeps the first binding registered for an action name,
/// so a changed configuration has to update KGlobalAccel explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutBinding {
    pub action: String,
    pub friendly: String,
    pub keys: Vec<i32>,
}

/// An update the host prepares before committing a configuration change.
pub struct Update {
    pub script: String,
    pub bindings: Vec<ShortcutBinding>,
}

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
    /// script. Configuration changes replace the script atomically and refresh
    /// the KGlobalAccel bindings, which KWin does not update on its own.
    pub fn sync(&mut self, config: &Config) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Err(error) = self.write_units() {
            warnings.push(format!("Wayland: {error}"));
        }
        let script = render(config);
        let unchanged = matches!(fs::read_to_string(&self.script_path), Ok(current) if current == script)
            && self.loaded;
        if !unchanged {
            if let Err(error) = self.write_script(&script) {
                warnings.push(format!("Wayland: {error}"));
                return warnings;
            }
            if let Err(error) = self.reload() {
                warnings.push(format!("Wayland: {error}"));
            }
        }
        warnings.extend(self.sync_bindings(&desired_bindings(config)));
        warnings
    }

    pub fn sync_script(&mut self, script: &str, bindings: &[ShortcutBinding]) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Err(error) = self.write_script(script) {
            warnings.push(format!("Wayland: {error}"));
            return warnings;
        }
        if let Err(error) = self.reload() {
            warnings.push(format!("Wayland: {error}"));
        }
        warnings.extend(self.sync_bindings(bindings));
        warnings
    }

    /// KWin keeps the binding an action had when it was first registered, so
    /// reloading the script never applies a changed shortcut. Update
    /// KGlobalAccel directly and report when an action is missing.
    fn sync_bindings(&self, bindings: &[ShortcutBinding]) -> Vec<String> {
        match update_bindings(bindings) {
            Ok(warnings) => warnings,
            Err(error) => vec![format!("Wayland: {error}")],
        }
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

/// The bindings the generated script owns. Mirrors `render`: the launcher is
/// only registered with a non-empty shortcut; every enabled profile on this
/// platform keeps an action even without a shortcut so a stale binding can be
/// cleared.
pub fn desired_bindings(config: &Config) -> Vec<ShortcutBinding> {
    let mut bindings = Vec::new();
    if !config.projects.hotkey.is_empty()
        && let Some(keys) = qt_keys(&config.projects.hotkey)
    {
        bindings.push(ShortcutBinding {
            action: action_title("projects"),
            friendly: "Sean Toolbox: recent projects".into(),
            keys,
        });
    }
    if config.windows.enabled {
        for profile in &config.windows.profiles {
            if !profile.enabled || profile.platform != Platform::current() {
                continue;
            }
            bindings.push(ShortcutBinding {
                action: action_title(&profile.id),
                friendly: format!("Sean Toolbox: {}", profile.name),
                keys: qt_keys(&profile.hotkey).unwrap_or_default(),
            });
        }
    }
    bindings
}

/// Render a shortcut as a KGlobalAccel/Qt key sequence value. The KWin script
/// passes the same text to `registerShortcut`, but KWin only applies it when
/// the action is first created.
fn qt_keys(text: &str) -> Option<Vec<i32>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    let key = HotkeySpec::parse(text).ok()?;
    let code = match key.key.as_str() {
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "ENTER" => 0x0d,
        "ESCAPE" => 0x1b,
        name if name.len() == 1 => i32::from(name.as_bytes()[0]),
        name => {
            let number: i32 = name.strip_prefix('F')?.parse().ok()?;
            if !(1..=24).contains(&number) {
                return None;
            }
            0x0100_0030 + number - 1
        }
    };
    let mut modifiers = 0;
    if key.shift {
        modifiers |= 0x0200_0000;
    }
    if key.ctrl {
        modifiers |= 0x0400_0000;
    }
    if key.alt {
        modifiers |= 0x0800_0000;
    }
    if key.super_key {
        modifiers |= 0x1000_0000;
    }
    Some(vec![code | modifiers])
}

type ShortcutInfo = (
    String,
    String,
    String,
    String,
    String,
    String,
    Vec<i32>,
    Vec<i32>,
);

fn shortcut_infos(connection: &Connection) -> Result<Vec<ShortcutInfo>, String> {
    let proxy = Proxy::new(
        connection,
        "org.kde.kglobalaccel",
        "/component/kwin",
        "org.kde.kglobalaccel.Component",
    )
    .map_err(|error| format!("Cannot reach KGlobalAccel: {error}"))?;
    proxy
        .call::<_, _, Vec<ShortcutInfo>>("allShortcutInfos", &())
        .map_err(|error| format!("Cannot read KGlobalAccel shortcuts: {error}"))
}

fn update_bindings(bindings: &[ShortcutBinding]) -> Result<Vec<String>, String> {
    if bindings.is_empty() {
        return Ok(Vec::new());
    }
    let connection = Connection::session()
        .map_err(|error| format!("Cannot connect to the session bus: {error}"))?;
    let mut infos = shortcut_infos(&connection)?;
    let missing = |infos: &[ShortcutInfo], binding: &ShortcutBinding| {
        !infos
            .iter()
            .any(|info| info.2 == KWIN_COMPONENT && info.0 == binding.action)
    };
    if bindings.iter().any(|binding| missing(&infos, binding)) {
        // KWin may still be executing the script; retry once before warning.
        std::thread::sleep(std::time::Duration::from_millis(100));
        infos = shortcut_infos(&connection)?;
    }
    let global = Proxy::new(
        &connection,
        "org.kde.kglobalaccel",
        "/kglobalaccel",
        "org.kde.KGlobalAccel",
    )
    .map_err(|error| format!("Cannot reach KGlobalAccel: {error}"))?;
    let mut warnings = Vec::new();
    for binding in bindings {
        let Some(info) = infos
            .iter()
            .find(|info| info.2 == KWIN_COMPONENT && info.0 == binding.action)
        else {
            warnings.push(format!(
                "Wayland: shortcut {} is not registered",
                binding.action
            ));
            continue;
        };
        if info.6 == binding.keys {
            continue;
        }
        let action_id = vec![
            info.2.clone(),
            info.0.clone(),
            info.3.clone(),
            info.1.clone(),
        ];
        if let Err(error) =
            global.call::<_, _, ()>("setForeignShortcut", &(action_id, binding.keys.clone()))
        {
            warnings.push(format!(
                "Wayland: cannot set shortcut {}: {error}",
                binding.action
            ));
        }
    }
    Ok(warnings)
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
    fn script_keeps_the_launcher_panel_out_of_task_switchers() {
        let script = render(&Config::default());
        assert!(script.contains("const LAUNCHER_CAPTION = 'Sean Toolbox · ';"));
        assert!(script.contains("window.skipTaskbar = true;"));
        assert!(script.contains("window.skipSwitcher = true;"));
    }

    #[test]
    fn action_titles_are_stable_for_invoke() {
        assert_eq!(action_title("vscode"), "Sean Toolbox: vscode");
    }

    #[test]
    fn qt_key_values_match_kglobalaccel() {
        assert_eq!(qt_keys("Alt+C"), Some(vec![134_217_795]));
        assert_eq!(qt_keys("F5"), Some(vec![16_777_268]));
        assert_eq!(qt_keys("Ctrl+Alt+Space"), Some(vec![201_326_624]));
        assert_eq!(qt_keys(""), Some(Vec::new()));
        assert_eq!(qt_keys("Ctrl+Alt+Unknown"), None);
    }

    #[test]
    fn desired_bindings_mirror_the_generated_script() {
        let config = Config::default();
        let bindings = desired_bindings(&config);
        assert_eq!(bindings.len(), 4);
        let launcher = bindings
            .iter()
            .find(|binding| binding.action == "Sean Toolbox: projects")
            .unwrap();
        assert_eq!(launcher.keys, vec![201_326_624]);
        let terminal = bindings
            .iter()
            .find(|binding| binding.action == "Sean Toolbox: terminal")
            .unwrap();
        assert!(terminal.keys.is_empty());
        let vscode = bindings
            .iter()
            .find(|binding| binding.action == "Sean Toolbox: vscode")
            .unwrap();
        assert_eq!(vscode.friendly, "Sean Toolbox: VS Code");
        assert_eq!(vscode.keys, vec![16_777_267]);
    }

    #[test]
    fn systemd_executable_is_quoted_safely() {
        assert_eq!(
            systemd_escape(Path::new("/opt/Sean 100%/sean-toolbox")),
            "/opt/Sean 100%%/sean-toolbox"
        );
    }
}
