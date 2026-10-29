use std::path::PathBuf;
use std::process::{Command, Stdio};
use toolbox_core::LaunchCommand;

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
