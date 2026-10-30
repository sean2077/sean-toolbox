use std::borrow::Cow;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use toolbox_core::{AppProfile, LaunchCommand};

pub(crate) const WEZTERM_QUAKE_CLASS_PREFIX: &str = "org.sean2077.sean-toolbox.quake.";

/// The persisted profile remains the user's launch configuration. Instance
/// isolation is applied only to Windows WezTerm Quake requests at runtime.
pub(crate) fn prepare_profile(profile: &AppProfile) -> Result<Cow<'_, AppProfile>, String> {
    if !profile.quake.enabled || !profile.is_windows_wezterm() {
        return Ok(Cow::Borrowed(profile));
    }
    let class = format!(
        "{WEZTERM_QUAKE_CLASS_PREFIX}{}",
        profile.id.to_ascii_lowercase()
    );
    let mut prepared = profile.clone();
    prepared.matcher = format!("class:{class}");
    for command in &mut prepared.launch {
        command.args = wezterm_arguments(&command.args, &class)?;
        // Use the GUI executable next to an explicitly configured CLI path;
        // bare names still use the normal installation/PATH resolver.
        let basename = command.executable.rfind(['/', '\\']).map_or(0, |i| i + 1);
        command
            .executable
            .replace_range(basename.., "wezterm-gui.exe");
    }
    Ok(Cow::Owned(prepared))
}

fn wezterm_arguments(args: &[String], class: &str) -> Result<Vec<String>, String> {
    let invalid = || {
        "WezTerm 下拉模式使用 start 启动终端；请检查启动参数 / WezTerm dropdown requires the start command".to_owned()
    };
    let mut result = Vec::new();
    let mut cursor = 0;
    // Global options belong before `start`. Preserve user configuration files
    // and unrelated overrides, including their original token boundaries.
    while let Some(arg) = args.get(cursor) {
        match arg.as_str() {
            "start" | "-e" => {
                cursor += 1;
                break;
            }
            "--config" | "--config-file" => {
                let value = args.get(cursor + 1).ok_or_else(invalid)?;
                if arg != "--config" || !managed_wezterm_override(value) {
                    result.extend([arg.clone(), value.clone()]);
                }
                cursor += 2;
            }
            "--skip-config" | "-n" => {
                result.push(arg.clone());
                cursor += 1;
            }
            _ if arg.starts_with("--config=") => {
                if !managed_wezterm_override(&arg[9..]) {
                    result.push(arg.clone());
                }
                cursor += 1;
            }
            _ if arg.starts_with("--config-file=") => {
                result.push(arg.clone());
                cursor += 1;
            }
            _ => return Err(invalid()),
        }
    }
    result.extend([
        "--config".into(),
        "window_decorations='RESIZE'".into(),
        "--config".into(),
        "adjust_window_size_when_changing_font_size=false".into(),
        "start".into(),
        "--always-new-process".into(),
        "--class".into(),
        class.into(),
    ]);
    // Only interpret start options. After `--` or the first program argument,
    // shell arguments (including --class or --new-tab) pass through unchanged.
    while let Some(arg) = args.get(cursor) {
        match arg.as_str() {
            "--class" => {
                args.get(cursor + 1).ok_or_else(invalid)?;
                cursor += 2;
            }
            "--always-new-process" | "--new-tab" => cursor += 1,
            _ if arg.starts_with("--class=") => cursor += 1,
            "--cwd" | "--position" | "--workspace" | "--domain" => {
                let value = args.get(cursor + 1).ok_or_else(invalid)?;
                result.extend([arg.clone(), value.clone()]);
                cursor += 2;
            }
            "--" => {
                result.extend_from_slice(&args[cursor..]);
                break;
            }
            _ if !arg.starts_with('-') => {
                result.extend_from_slice(&args[cursor..]);
                break;
            }
            _ => {
                result.push(arg.clone());
                cursor += 1;
            }
        }
    }
    Ok(result)
}

fn managed_wezterm_override(value: &str) -> bool {
    matches!(
        value.split_once('=').map(|(key, _)| key.trim()),
        Some("window_decorations" | "adjust_window_size_when_changing_font_size")
    )
}

pub(crate) fn expand_path(value: &str) -> String {
    let mut result = value.to_owned();
    if (result == "~" || result.starts_with("~/") || result.starts_with("~\\"))
        && let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
    {
        result = format!("{}{}", home.to_string_lossy(), &result[1..]);
    }
    // Expansion is deliberately restricted to paths, never argument strings.
    // No shell or command-line interpreter is involved.
    for (key, value) in std::env::vars_os() {
        if let (Some(key), Some(value)) = (key.to_str(), value.to_str()) {
            result = result
                .replace(&format!("%{key}%"), value)
                .replace(&format!("${{{key}}}"), value);
        }
    }
    result
}

#[cfg(windows)]
fn executable_path(executable: &str) -> String {
    if std::path::Path::new(executable).components().count() > 1 {
        return executable.into();
    }
    let paths: &[(&str, &str)] = match executable.to_ascii_lowercase().as_str() {
        "code.exe" => &[
            ("LOCALAPPDATA", "Programs\\Microsoft VS Code\\Code.exe"),
            ("ProgramFiles", "Microsoft VS Code\\Code.exe"),
        ],
        "chrome.exe" => &[
            ("ProgramFiles", "Google\\Chrome\\Application\\chrome.exe"),
            (
                "ProgramFiles(x86)",
                "Google\\Chrome\\Application\\chrome.exe",
            ),
            ("LOCALAPPDATA", "Google\\Chrome\\Application\\chrome.exe"),
        ],
        "wezterm-gui.exe" => &[
            ("ProgramFiles", "WezTerm\\wezterm-gui.exe"),
            ("LOCALAPPDATA", "Programs\\WezTerm\\wezterm-gui.exe"),
        ],
        _ => &[],
    };
    for (variable, relative) in paths {
        if let Some(base) = std::env::var_os(variable) {
            let path = PathBuf::from(base).join(relative);
            if path.is_file() {
                return path.to_string_lossy().into_owned();
            }
        }
    }
    executable.into()
}

pub(crate) fn spawn(command: &LaunchCommand) -> Result<(), String> {
    if command.executable.trim().is_empty()
        || command.executable.contains('\0')
        || command.args.iter().any(|a| a.contains('\0'))
    {
        return Err("Invalid launch command / 启动命令无效".into());
    }
    let executable = expand_path(&command.executable);
    let extension = PathBuf::from(&executable)
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_lowercase());
    if matches!(
        extension.as_deref(),
        Some("cmd" | "bat" | "ps1" | "vbs" | "js")
    ) {
        return Err(
            "Use an executable and a separate argument array / 请提供可执行文件及独立参数数组"
                .into(),
        );
    }
    #[cfg(windows)]
    let executable = executable_path(&executable);
    #[cfg(target_os = "macos")]
    let mut process = {
        if executable.ends_with(".app") {
            let mut p = Command::new("/usr/bin/open");
            p.args(["-a", &executable]);
            if !command.args.is_empty() {
                p.arg("--args");
            }
            p
        } else {
            Command::new(&executable)
        }
    };
    #[cfg(not(target_os = "macos"))]
    let mut process = Command::new(&executable);
    process
        .args(&command.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(cwd) = &command.cwd {
        process.current_dir(expand_path(cwd));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        process.creation_flags(0x0000_0008); // DETACHED_PROCESS; GUI apps need no inherited console.
    }
    #[cfg(unix)]
    let reaper = {
        // A sleeping wait thread reaps exactly one child with no periodic
        // wakeups. Its small stack avoids both zombie children and an idle
        // application-monitoring loop. Reserve it before spawning so a thread
        // allocation failure cannot leave a running child without a reaper.
        let (sender, receiver) = std::sync::mpsc::channel::<std::process::Child>();
        std::thread::Builder::new()
            .name("toolbox-reap".into())
            .stack_size(64 * 1024)
            .spawn(move || {
                if let Ok(mut child) = receiver.recv() {
                    let _ = child.wait();
                }
            })
            .map_err(|e| format!("Cannot create child reaper: {e}"))?;
        sender
    };
    let child = process.spawn().map_err(|e| format!("{executable}: {e}"))?;
    #[cfg(unix)]
    let _ = reaper.send(child);
    #[cfg(not(unix))]
    drop(child);
    // After successful spawn any later timeout is ambiguous. The engine must
    // not try a fallback executable or it may create a second instance.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toolbox_core::Platform;

    #[test]
    fn wezterm_dropdown_uses_a_dedicated_gui_without_mutating_saved_settings() {
        let mut profile = AppProfile::wezterm(Platform::Windows);
        profile.id = "My_Terminal".into();
        profile.launch[0].executable = r"D:\Portable Apps\WezTerm\wezterm.exe".into();
        let saved = profile.clone();
        let prepared = prepare_profile(&profile).unwrap();
        assert_eq!(profile, saved);
        assert_eq!(
            prepared.matcher,
            "class:org.sean2077.sean-toolbox.quake.my_terminal"
        );
        assert_eq!(
            prepared.launch[0].executable,
            r"D:\Portable Apps\WezTerm\wezterm-gui.exe"
        );
        assert_eq!(
            prepared.launch[0].args,
            [
                "--config",
                "window_decorations='RESIZE'",
                "--config",
                "adjust_window_size_when_changing_font_size=false",
                "start",
                "--always-new-process",
                "--class",
                "org.sean2077.sean-toolbox.quake.my_terminal",
            ]
        );
        assert_eq!(
            prepare_profile(&prepared).unwrap().as_ref(),
            prepared.as_ref()
        );
    }

    #[test]
    fn wezterm_preserves_custom_config_working_directory_and_shell_argument_boundaries() {
        let mut profile = AppProfile::wezterm(Platform::Windows);
        profile.launch[0].cwd = Some(r"D:\My Work".into());
        profile.launch[0].args = [
            "--config-file",
            r"D:\My Config\wezterm.lua",
            "--config",
            "font_size=13",
            "--config=window_decorations='NONE'",
            "--config",
            "adjust_window_size_when_changing_font_size=true",
            "start",
            "--class",
            "old-class",
            "--new-tab",
            "--always-new-process",
            "--cwd",
            r"D:\My Work",
            "--workspace=agents",
            "--",
            r"C:\Program Files\Git\bin\bash.exe",
            "-lc",
            "printf '%s' '--class --new-tab $HOME'",
            "--class",
            "shell-option",
        ]
        .map(String::from)
        .into();
        let prepared = prepare_profile(&profile).unwrap();
        assert_eq!(prepared.launch[0].cwd, profile.launch[0].cwd);
        assert_eq!(
            prepared.launch[0].args,
            [
                "--config-file",
                r"D:\My Config\wezterm.lua",
                "--config",
                "font_size=13",
                "--config",
                "window_decorations='RESIZE'",
                "--config",
                "adjust_window_size_when_changing_font_size=false",
                "start",
                "--always-new-process",
                "--class",
                "org.sean2077.sean-toolbox.quake.terminal",
                "--cwd",
                r"D:\My Work",
                "--workspace=agents",
                "--",
                r"C:\Program Files\Git\bin\bash.exe",
                "-lc",
                "printf '%s' '--class --new-tab $HOME'",
                "--class",
                "shell-option",
            ]
        );
    }

    #[test]
    fn wezterm_start_alias_and_program_arguments_are_preserved() {
        for start in ["start", "-e"] {
            let args = [start, "bash", "-lc", "echo hi", "--class=not-a-window"].map(String::from);
            let prepared = wezterm_arguments(&args, "dedicated").unwrap();
            assert_eq!(
                &prepared[8..],
                ["bash", "-lc", "echo hi", "--class=not-a-window"]
            );
        }
        let args = ["--skip-config", "--config-file=custom.lua"].map(String::from);
        let prepared = wezterm_arguments(&args, "dedicated").unwrap();
        assert_eq!(&prepared[..2], args);
        assert_eq!(
            &prepared[6..],
            ["start", "--always-new-process", "--class", "dedicated"]
        );
    }

    #[test]
    fn unsupported_wezterm_commands_and_incomplete_options_fail_before_launch() {
        for args in [
            vec!["cli", "list"],
            vec!["ssh", "host"],
            vec!["connect", "remote"],
            vec!["--config"],
            vec!["--config-file"],
            vec!["start", "--class"],
            vec!["start", "--cwd"],
        ] {
            let mut profile = AppProfile::wezterm(Platform::Windows);
            profile.launch[0].args = args.iter().map(|s| (*s).to_owned()).collect();
            assert!(prepare_profile(&profile).is_err(), "{args:?}");
        }
    }

    #[test]
    fn ordinary_and_other_platform_profiles_keep_their_launch_contract() {
        for platform in [Platform::Windows, Platform::Macos, Platform::Linux] {
            let mut profile = AppProfile::wezterm(platform);
            profile.quake.enabled = platform != Platform::Windows;
            assert!(matches!(
                prepare_profile(&profile).unwrap(),
                Cow::Borrowed(_)
            ));
        }
        let mut profile = AppProfile::wezterm(Platform::Windows);
        profile.launch.push(LaunchCommand {
            executable: "other-terminal.exe".into(),
            args: vec![],
            cwd: None,
        });
        assert!(matches!(
            prepare_profile(&profile).unwrap(),
            Cow::Borrowed(_)
        ));
    }
}
