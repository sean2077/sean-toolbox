//! Remote-control session detection for automatic clipboard suspension.
//!
//! A second clipboard synchronizer on the same desktop (UU Remote, ToDesk,
//! AnyDesk, RustDesk, a VNC server) overwrites the same Wayland selection this
//! bridge uses. When one is active, the bridge stops reading and writing the
//! clipboard so the two do not keep replaying each other's content. Detection
//! is deliberately simple and configurable: a process name list, plus an extra
//! child-process check for UU Remote, whose host process is long-lived even
//! when nobody is connected.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// Names matched case-insensitively against the executable basename (`comm`
/// on Linux, `szExeFile` on Windows). Deskflow, Synergy and KDE Connect are
/// intentionally absent: they are long-lived local tools, and their clipboard
/// feature should be switched off directly instead of pausing this bridge.
pub const DEFAULT_REMOTE_PROCESSES: &[&str] = &[
    "uuremote-server",
    "uuremote-viewer",
    "GameViewer.exe",
    "todesk",
    "anydesk",
    "rustdesk",
    "sunshine",
    "teamviewer",
    "wayvnc",
    "krfb",
    "x11vnc",
    "parsecd.exe",
];

/// Automatic suspension settings. The library default keeps detection off;
/// the application enables it from `config.json` so tests and embedders are
/// not affected by remote-control processes on the build machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuspendSettings {
    pub auto: bool,
    pub processes: Vec<String>,
    /// Child processes that make a long-lived host count as an active session.
    /// UU Remote starts `ffmpeg` only while a viewer is connected.
    pub uu_child: Vec<String>,
    /// Wait this long after the session disappears, so a process restart or a
    /// codec switch does not resume the bridge in the middle of a session.
    pub recover_delay: Duration,
}

impl Default for SuspendSettings {
    fn default() -> Self {
        Self {
            auto: false,
            processes: DEFAULT_REMOTE_PROCESSES
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            uu_child: vec!["ffmpeg".into()],
            recover_delay: Duration::from_secs(3),
        }
    }
}

/// Detect a session synchronously, so the service can start paused before the
/// backend's initial clipboard event reaches the worker.
pub fn detect_now(settings: &SuspendSettings) -> Option<String> {
    if !settings.auto {
        return None;
    }
    detect(settings, &processes())
}

/// Watch for an active remote-control session until `stop` is set.
///
/// `on_change` is called only when the detected process changes; it receives
/// the configured process name, or `None` when no session remains.
pub fn watch(
    settings: SuspendSettings,
    stop: Arc<AtomicBool>,
    on_change: impl Fn(Option<String>) + Send + 'static,
) {
    let _ = thread::Builder::new()
        .name("toolbox-clipboard-remote".into())
        .spawn(move || {
            let mut last: Option<String> = None;
            loop {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                let current = detect(&settings, &processes());
                if current != last {
                    last = current.clone();
                    on_change(current);
                }
                // Sleep in short steps so stopping stays responsive.
                let until = Instant::now() + Duration::from_secs(2);
                while Instant::now() < until && !stop.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(200));
                }
            }
        });
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessInfo {
    pid: u32,
    parent: u32,
    name: String,
}

fn detect(settings: &SuspendSettings, processes: &[ProcessInfo]) -> Option<String> {
    if !settings.auto {
        return None;
    }
    for wanted in &settings.processes {
        let matching: Vec<&ProcessInfo> = processes
            .iter()
            .filter(|process| process.name.eq_ignore_ascii_case(wanted))
            .collect();
        if matching.is_empty() {
            continue;
        }
        if wanted.eq_ignore_ascii_case("uuremote-server") && !settings.uu_child.is_empty() {
            let active = matching.iter().any(|parent| {
                processes.iter().any(|child| {
                    child.parent == parent.pid
                        && settings
                            .uu_child
                            .iter()
                            .any(|name| child.name.eq_ignore_ascii_case(name))
                })
            });
            if active {
                return Some(wanted.clone());
            }
            continue;
        }
        return Some(wanted.clone());
    }
    None
}

fn processes() -> Vec<ProcessInfo> {
    #[cfg(target_os = "linux")]
    {
        linux_processes()
    }
    #[cfg(windows)]
    {
        windows_processes()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn linux_processes() -> Vec<ProcessInfo> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(comm) = std::fs::read_to_string(entry.path().join("comm")) else {
            continue;
        };
        let name = comm.trim_end().to_owned();
        if name.is_empty() {
            continue;
        }
        let parent = std::fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|stat| parse_parent(&stat))
            .unwrap_or(0);
        result.push(ProcessInfo { pid, parent, name });
    }
    result
}

/// The second field after the parenthesised command is the parent PID. The
/// command itself may contain spaces or parentheses, so split after the last
/// `)` rather than on the first one.
#[cfg(target_os = "linux")]
fn parse_parent(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    let _state = fields.next()?;
    fields.next()?.parse().ok()
}

#[cfg(windows)]
fn windows_processes() -> Vec<ProcessInfo> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };

    let mut result = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return result;
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = Process32FirstW(snapshot, &mut entry) != 0;
        while found {
            let end = entry
                .szExeFile
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if !name.is_empty() {
                result.push(ProcessInfo {
                    pid: entry.th32ProcessID,
                    parent: entry.th32ParentProcessID,
                    name,
                });
            }
            found = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, parent: u32, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent,
            name: name.into(),
        }
    }

    fn enabled() -> SuspendSettings {
        SuspendSettings {
            auto: true,
            processes: vec!["uuremote-server".into(), "todesk".into()],
            uu_child: vec!["ffmpeg".into()],
            recover_delay: Duration::from_secs(3),
        }
    }

    #[test]
    fn uu_host_needs_a_live_encoder_child() {
        let idle = vec![process(1, 0, "systemd"), process(42, 1, "uuremote-server")];
        assert_eq!(detect(&enabled(), &idle), None);

        let streaming = vec![
            process(1, 0, "systemd"),
            process(42, 1, "uuremote-server"),
            process(43, 42, "ffmpeg"),
        ];
        assert_eq!(
            detect(&enabled(), &streaming),
            Some("uuremote-server".into())
        );
    }

    #[test]
    fn a_plain_remote_process_matches_by_name() {
        let processes = vec![process(7, 1, "ToDesk")];
        assert_eq!(detect(&enabled(), &processes), Some("todesk".into()));
    }

    #[test]
    fn automatic_detection_can_be_turned_off() {
        let mut settings = enabled();
        settings.auto = false;
        assert_eq!(
            detect(
                &settings,
                &[process(42, 1, "uuremote-server"), process(43, 42, "ffmpeg")]
            ),
            None
        );
    }

    #[test]
    fn an_empty_uu_child_list_matches_the_host_itself() {
        let mut settings = enabled();
        settings.uu_child.clear();
        assert_eq!(
            detect(&settings, &[process(42, 1, "uuremote-server")]),
            Some("uuremote-server".into())
        );
    }
}
