use std::path::Path;

pub fn set(enabled: bool, executable: &Path) -> Result<(), String> {
    platform_set(enabled, executable).map_err(|e| format!("开机启动设置失败 / Autostart: {e}"))
}

#[cfg(windows)]
fn platform_set(enabled: bool, executable: &Path) -> Result<(), String> {
    use std::{
        os::windows::ffi::OsStrExt,
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        System::Registry::*,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
    let path = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = wide("SeanToolbox");
    let mut key = null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("Registry error {status}"));
    }
    let status = if enabled {
        let mut value: Vec<u16> = vec![b'"' as u16];
        value.extend(executable.as_os_str().encode_wide());
        value.extend("\" --background".encode_utf16());
        value.push(0);
        unsafe {
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        }
    } else {
        unsafe { RegDeleteValueW(key, name.as_ptr()) }
    };
    unsafe {
        RegCloseKey(key);
    }
    if status == ERROR_SUCCESS || (!enabled && status == ERROR_FILE_NOT_FOUND) {
        Ok(())
    } else {
        Err(format!("Registry error {status}"))
    }
}

#[cfg(target_os = "macos")]
fn platform_set(enabled: bool, executable: &Path) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
    let path =
        std::path::PathBuf::from(home).join("Library/LaunchAgents/com.sean2077.toolbox.plist");
    if !enabled {
        return remove_if_exists(&path);
    }
    let exe = executable
        .to_str()
        .ok_or("Executable path is not Unicode")?;
    let escaped = exe
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;");
    write_entry(
        &path,
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>com.sean2077.toolbox</string><key>ProgramArguments</key><array><string>{escaped}</string><string>--background</string></array><key>RunAtLoad</key><true/></dict></plist>\n"
        ),
    )
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_set(enabled: bool, executable: &Path) -> Result<(), String> {
    use std::{env, path::PathBuf};
    let root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .ok_or("HOME is unavailable")?;
    let path = root.join("autostart/sean-toolbox.desktop");
    if !enabled {
        return remove_if_exists(&path);
    }
    let exe = executable
        .to_str()
        .ok_or("Executable path is not Unicode")?;
    if exe.contains(['\n', '\r']) {
        return Err("Executable path contains a line break".into());
    }
    // Desktop Entry Exec quoting has two escaping layers; this is never a shell command.
    let quoted = exe
        .replace('\\', "\\\\\\\\")
        .replace('"', "\\\\\"")
        .replace('`', "\\\\`")
        .replace('$', "\\\\$")
        .replace('%', "%%");
    write_entry(
        &path,
        &format!(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Sean Toolbox\nComment=个人工具箱\nExec=\"{quoted}\" --background\nTerminal=false\nCategories=Utility;\n"
        ),
    )
}

#[cfg(not(windows))]
fn remove_if_exists(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(windows))]
fn write_entry(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("Missing autostart directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.error.to_string())?;
    Ok(())
}
