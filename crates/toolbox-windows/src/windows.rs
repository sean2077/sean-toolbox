//! Win32 backend. Requests ordinary foreground activation; it never injects
//! keys, attaches input threads, or changes foreground-lock policy.
use crate::backend::{Desktop, Window};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub(crate) struct WindowsDesktop {
    restore_maximized: HashMap<String, bool>,
}

impl WindowsDesktop {
    pub(crate) fn new() -> Result<Self, String> {
        let mut desktop = Self {
            restore_maximized: HashMap::new(),
        };
        desktop.check_available()?;
        Ok(desktop)
    }
    fn pid(hwnd: HWND) -> u32 {
        let mut pid = 0;
        // SAFETY: a valid writable integer is supplied; Win32 accepts stale HWNDs.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        pid
    }
    fn process(pid: u32) -> Option<(String, u64)> {
        // SAFETY: read-only process-query access. The returned handle is closed
        // in all paths before this function returns.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let mut path = vec![0u16; 32768];
            let mut size = path.len() as u32;
            let mut created: FILETIME = std::mem::zeroed();
            let mut exited: FILETIME = std::mem::zeroed();
            let mut kernel: FILETIME = std::mem::zeroed();
            let mut user: FILETIME = std::mem::zeroed();
            let readable = QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut size) != 0
                && GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0;
            CloseHandle(handle);
            readable.then(|| {
                (
                    String::from_utf16_lossy(&path[..(size as usize).min(path.len())]),
                    (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
                )
            })
        }
    }
    fn identity(hwnd: HWND) -> Option<String> {
        // SAFETY: Win32 safely checks even a stale or foreign-process HWND.
        if hwnd.is_null() || unsafe { IsWindow(hwnd) } == 0 {
            return None;
        }
        let pid = Self::pid(hwnd);
        if pid == 0 {
            return None;
        }
        let start = Self::process(pid).map(|(_, t)| t).unwrap_or(0);
        Some(format!("win:{}:{pid}:{start}", hwnd as usize))
    }
    fn validate(&self, id: &str) -> Result<HWND, String> {
        let handle: usize = id
            .strip_prefix("win:")
            .and_then(|s| s.split(':').next())
            .and_then(|s| s.parse().ok())
            .ok_or("Invalid Windows window identifier")?;
        let hwnd = handle as HWND;
        if Self::identity(hwnd).as_deref() != Some(id) {
            return Err(
                "窗口已关闭、句柄已变化或权限不足 / Window is stale or inaccessible".into(),
            );
        }
        Ok(hwnd)
    }
    fn placement(hwnd: HWND) -> Option<WINDOWPLACEMENT> {
        // SAFETY: structure length is initialized per GetWindowPlacement's contract.
        unsafe {
            let mut placement: WINDOWPLACEMENT = std::mem::zeroed();
            placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
            (GetWindowPlacement(hwnd, &mut placement) != 0).then_some(placement)
        }
    }
    fn wait_for(
        &self,
        id: &str,
        action: &str,
        predicate: impl Fn(HWND) -> bool,
    ) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(750);
        loop {
            if predicate(self.validate(id)?) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "窗口未完成{action}，应用可能无响应或权限不足 / Window did not acknowledge {action}"
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Desktop for WindowsDesktop {
    fn check_available(&mut self) -> Result<(), String> {
        // SAFETY: GetShellWindow has no pointer arguments or caller-owned state.
        if unsafe { GetShellWindow() }.is_null() {
            return Err(
                "未发现交互式 Windows 桌面 / No interactive Windows shell is available".into(),
            );
        }
        Ok(())
    }
    fn windows(&mut self, matcher: &str) -> Result<Vec<Window>, String> {
        unsafe extern "system" fn collect(hwnd: HWND, state: LPARAM) -> i32 {
            // SAFETY: EnumWindows invokes this synchronously with the pointer to
            // the live Vec supplied below. The callback cannot outlive it.
            unsafe {
                (&mut *(state as *mut Vec<HWND>)).push(hwnd);
            }
            1
        }
        let mut handles = Vec::<HWND>::new();
        // SAFETY: the callback only writes to `handles` during this synchronous call.
        if unsafe { EnumWindows(Some(collect), (&mut handles as *mut Vec<HWND>) as LPARAM) } == 0 {
            return Err(format!("EnumWindows: {}", std::io::Error::last_os_error()));
        }
        let mut process_cache = HashMap::new();
        let mut unreadable = HashSet::new();
        let mut live = HashSet::new();
        let mut windows = Vec::new();
        let wanted = Path::new(matcher)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(matcher);
        for hwnd in handles {
            // SAFETY: all Win32 calls accept arbitrary HWND values and return
            // failure for windows destroyed during enumeration.
            unsafe {
                if IsWindow(hwnd) == 0 || IsWindowVisible(hwnd) == 0 {
                    continue;
                }
                let pid = Self::pid(hwnd);
                if pid == 0 {
                    continue;
                }
                #[cfg(target_pointer_width = "64")]
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                #[cfg(target_pointer_width = "32")]
                let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                if style & WS_EX_APPWINDOW == 0
                    && (style & WS_EX_TOOLWINDOW != 0 || !GetWindow(hwnd, GW_OWNER).is_null())
                {
                    continue;
                }
                let mut cloaked = 0u32;
                if DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_CLOAKED as u32,
                    (&mut cloaked as *mut u32).cast(),
                    std::mem::size_of::<u32>() as u32,
                ) == 0
                    && cloaked != 0
                {
                    continue;
                }
                let count = (GetWindowTextLengthW(hwnd).max(0) as usize + 1).min(32768);
                let mut title = vec![0u16; count];
                let copied = GetWindowTextW(hwnd, title.as_mut_ptr(), count as i32).max(0) as usize;
                let title = String::from_utf16_lossy(&title[..copied.min(count)]);
                if title.trim().is_empty() {
                    continue;
                }
                let process = process_cache
                    .entry(pid)
                    .or_insert_with(|| Self::process(pid));
                if IsWindow(hwnd) == 0 || Self::pid(hwnd) != pid {
                    continue;
                }
                let Some((executable, start)) = process else {
                    unreadable.insert(pid);
                    continue;
                };
                let id = format!("win:{}:{pid}:{start}", hwnd as usize);
                live.insert(id.clone());
                if !Path::new(executable)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or(executable)
                    .eq_ignore_ascii_case(wanted)
                {
                    continue;
                }
                let minimized = IsIconic(hwnd) != 0;
                let maximized = IsZoomed(hwnd) != 0
                    || (minimized
                        && (self.restore_maximized.get(&id) == Some(&true)
                            || Self::placement(hwnd)
                                .is_some_and(|p| p.flags & WPF_RESTORETOMAXIMIZED != 0)));
                windows.push(Window {
                    id,
                    title,
                    minimized,
                    maximized,
                });
            }
        }
        self.restore_maximized.retain(|id, _| live.contains(id));
        if windows.is_empty() && !unreadable.is_empty() {
            return Err(format!(
                "无法确认 {matcher} 是否已打开：{} 个可见窗口的进程信息无法读取。请检查应用运行权限 / Cannot safely determine whether the application is running",
                unreadable.len()
            ));
        }
        Ok(windows)
    }
    fn foreground(&mut self) -> Result<Option<String>, String> {
        // Identity remains available (start-time 0) even for an elevated
        // foreground process, so launch focus guards still notice a switch.
        Ok(Self::identity(unsafe { GetForegroundWindow() }))
    }
    fn minimize(&mut self, id: &str) -> Result<(), String> {
        let hwnd = self.validate(id)?;
        // SAFETY: the handle was revalidated immediately above.
        unsafe {
            if IsIconic(hwnd) != 0 {
                return Ok(());
            }
            self.restore_maximized
                .insert(id.to_owned(), IsZoomed(hwnd) != 0);
            if ShowWindowAsync(hwnd, SW_MINIMIZE) == 0 {
                return Err(format!(
                    "ShowWindowAsync: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        self.wait_for(id, "最小化", |h| unsafe { IsIconic(h) != 0 })
    }
    fn restore(&mut self, id: &str) -> Result<(), String> {
        let hwnd = self.validate(id)?;
        let maximize = self.restore_maximized.get(id) == Some(&true)
            || Self::placement(hwnd).is_some_and(|p| p.flags & WPF_RESTORETOMAXIMIZED != 0);
        unsafe {
            if IsIconic(hwnd) == 0 {
                return Ok(());
            }
            if ShowWindowAsync(hwnd, if maximize { SW_MAXIMIZE } else { SW_RESTORE }) == 0 {
                return Err(format!(
                    "ShowWindowAsync: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        self.wait_for(id, "恢复", |h| unsafe {
            IsIconic(h) == 0 && (!maximize || IsZoomed(h) != 0)
        })?;
        self.restore_maximized.remove(id);
        Ok(())
    }
    fn maximize(&mut self, id: &str) -> Result<(), String> {
        let hwnd = self.validate(id)?;
        unsafe {
            if IsZoomed(hwnd) != 0 && IsIconic(hwnd) == 0 {
                return Ok(());
            }
            if ShowWindowAsync(hwnd, SW_MAXIMIZE) == 0 {
                return Err(format!(
                    "ShowWindowAsync: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        self.wait_for(id, "最大化", |h| unsafe {
            IsZoomed(h) != 0 && IsIconic(h) == 0
        })
    }
    fn activate(&mut self, id: &str) -> Result<bool, String> {
        let hwnd = self.validate(id)?;
        let original = unsafe { GetForegroundWindow() };
        if original == hwnd {
            return Ok(true);
        }
        // One ordinary request. A denial is reported honestly to the host.
        unsafe {
            SetForegroundWindow(hwnd);
        }
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            self.validate(id)?;
            let now = unsafe { GetForegroundWindow() };
            if now == hwnd {
                return Ok(true);
            }
            if (!now.is_null() && now != original) || Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}
