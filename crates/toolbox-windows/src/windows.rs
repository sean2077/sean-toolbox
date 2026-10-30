//! Win32 backend. Requests ordinary foreground activation; it never injects
//! keys, attaches input threads, or changes foreground-lock policy.
use crate::backend::{Desktop, Window, WindowRect};
use crate::launch::WEZTERM_QUAKE_CLASS_PREFIX;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};
use toolbox_core::{QuakeConfig, QuakeMonitor};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint, MonitorFromWindow,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub(crate) struct WindowsDesktop {
    restore_maximized: HashMap<String, bool>,
    quake_topmost: HashMap<String, bool>,
    quake_hidden: HashSet<String>,
}

impl WindowsDesktop {
    pub(crate) fn new() -> Result<Self, String> {
        let mut desktop = Self {
            restore_maximized: HashMap::new(),
            quake_topmost: HashMap::new(),
            quake_hidden: HashSet::new(),
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
    fn handle(id: &str) -> HWND {
        id.strip_prefix("win:")
            .and_then(|s| s.split(':').next())
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0) as HWND
    }
    fn validate(&self, id: &str) -> Result<HWND, String> {
        let hwnd = Self::handle(id);
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
    fn class(hwnd: HWND) -> Option<String> {
        let mut class = [0u16; 256];
        // SAFETY: GetClassNameW writes at most the supplied buffer length.
        let length = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
        (length > 0).then(|| String::from_utf16_lossy(&class[..length as usize]))
    }
    fn show_without_activation(&mut self, id: &str) -> Result<(), String> {
        let hwnd = self.validate(id)?;
        let was_hidden = unsafe { IsWindowVisible(hwnd) == 0 };
        // SW_SHOWNOACTIVATE restores a normal-size window without giving it
        // focus. In particular, cleanup must not reactivate a hidden terminal.
        // Post even if still visible: a timed-out hide may remain in the target
        // thread's queue, and must be followed by a recovery request.
        if unsafe { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) } == 0 {
            return Err(format!(
                "ShowWindowAsync: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.wait_for(id, "显示下拉窗口", |h| unsafe {
            IsWindowVisible(h) != 0 && IsIconic(h) == 0
        })?;
        // A still-visible window can have an earlier hide request pending in
        // its thread's queue. Current visibility does not acknowledge this
        // new show request; retain recovery state until a hidden-to-shown
        // transition is observed, so later reset/drop can retry it.
        if was_hidden {
            self.quake_hidden.remove(id);
        }
        Ok(())
    }
    fn restore_hidden(&mut self, id: &str) -> Result<(), String> {
        if !self.quake_hidden.contains(id) {
            return Ok(());
        }
        if let Err(error) = self.validate(id) {
            // A recycled or closed handle must never receive a saved operation.
            // Preserve cleanup state when only process-query access failed.
            let hwnd = Self::handle(id);
            let current = Self::identity(hwnd);
            let inaccessible = current.as_deref().is_some_and(|current| {
                current.strip_suffix(":0").is_some_and(|prefix| {
                    id.rsplit_once(':')
                        .is_some_and(|(saved, _)| saved == prefix)
                })
            });
            if inaccessible {
                return Err(error);
            }
            self.quake_hidden.remove(id);
            return Ok(());
        }
        self.show_without_activation(id)
    }
    fn topmost(hwnd: HWND) -> bool {
        // SAFETY: querying window style is safe for a stale HWND as well.
        #[cfg(target_pointer_width = "64")]
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
        #[cfg(target_pointer_width = "32")]
        let style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
        style & WS_EX_TOPMOST != 0
    }
    fn position(hwnd: HWND, bounds: WindowRect, move_only: bool) -> Result<(), String> {
        let flags = SWP_NOACTIVATE
            | SWP_NOZORDER
            | SWP_ASYNCWINDOWPOS
            | if move_only { SWP_NOSIZE } else { 0 };
        // SAFETY: coordinates were bounded by quake_bounds; Win32 validates HWND.
        if unsafe {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                bounds.x,
                bounds.y,
                bounds.width as i32,
                bounds.height as i32,
                flags,
            )
        } == 0
        {
            return Err(format!("SetWindowPos: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
    fn at_bounds(hwnd: HWND, bounds: WindowRect, position_only: bool) -> bool {
        let mut actual = RECT::default();
        if unsafe { GetWindowRect(hwnd, &mut actual) } == 0 {
            return false;
        }
        let close = |a: i64, b: i64| (a - b).abs() <= 4;
        close(i64::from(actual.left), i64::from(bounds.x))
            && close(i64::from(actual.top), i64::from(bounds.y))
            && (position_only
                || (close(
                    i64::from(actual.right) - i64::from(actual.left),
                    i64::from(bounds.width),
                ) && close(
                    i64::from(actual.bottom) - i64::from(actual.top),
                    i64::from(bounds.height),
                )))
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

impl Drop for WindowsDesktop {
    fn drop(&mut self) {
        let _ = self.clear_quake();
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
        let wanted_class = matcher.strip_prefix("class:");
        if wanted_class.is_some_and(str::is_empty) {
            return Err("A class matcher needs a window class / 窗口类名不能为空".into());
        }
        let mut handles = Vec::<HWND>::new();
        // SAFETY: the callback only writes to `handles` during this synchronous call.
        if unsafe { EnumWindows(Some(collect), (&mut handles as *mut Vec<HWND>) as LPARAM) } == 0 {
            return Err(format!("EnumWindows: {}", std::io::Error::last_os_error()));
        }
        let mut process_cache = HashMap::new();
        let mut unreadable = HashSet::new();
        let mut windows = Vec::new();
        let wanted = Path::new(matcher)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(matcher);
        let hidden_handles = self
            .quake_hidden
            .iter()
            .filter_map(|id| {
                id.strip_prefix("win:")?
                    .split(':')
                    .next()?
                    .parse::<usize>()
                    .ok()
            })
            .collect::<HashSet<_>>();
        for hwnd in handles {
            // SAFETY: all Win32 calls accept arbitrary HWND values and return
            // failure for windows destroyed during enumeration.
            unsafe {
                if IsWindow(hwnd) == 0 {
                    continue;
                }
                let class = Self::class(hwnd).unwrap_or_default();
                if let Some(wanted_class) = wanted_class {
                    // Filter before querying foreign processes. An unrelated
                    // elevated window cannot prevent finding this dedicated one.
                    if !class.eq_ignore_ascii_case(wanted_class) {
                        continue;
                    }
                } else if class
                    .to_ascii_lowercase()
                    .starts_with(WEZTERM_QUAKE_CLASS_PREFIX)
                {
                    // Ordinary WezTerm shortcuts must not capture its dropdown.
                    continue;
                }
                let hidden = IsWindowVisible(hwnd) == 0;
                if hidden && wanted_class.is_none() && !hidden_handles.contains(&(hwnd as usize)) {
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
                let auxiliary =
                    style & WS_EX_TOOLWINDOW != 0 || !GetWindow(hwnd, GW_OWNER).is_null();
                if auxiliary && (wanted_class.is_some() || style & WS_EX_APPWINDOW == 0) {
                    // WezTerm also uses its class for owned dialogs; those are
                    // part of the terminal's focus family, not docking targets.
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
                    && wanted_class.is_none()
                {
                    continue;
                }
                let count = (GetWindowTextLengthW(hwnd).max(0) as usize + 1).min(32768);
                let mut title = vec![0u16; count];
                let copied = GetWindowTextW(hwnd, title.as_mut_ptr(), count as i32).max(0) as usize;
                let title = String::from_utf16_lossy(&title[..copied.min(count)]);
                if title.trim().is_empty() && wanted_class.is_none() && !hidden {
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
                if hidden && wanted_class.is_none() && !self.quake_hidden.contains(&id) {
                    continue;
                }
                if wanted_class.is_none()
                    && !Path::new(executable)
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
                    title: if title.trim().is_empty() {
                        class
                    } else {
                        title
                    },
                    minimized,
                    maximized,
                    hidden,
                });
            }
        }
        // Class-filtered enumeration is not a complete application inventory.
        // Revalidate saved identities rather than deleting other profiles' state.
        self.restore_maximized
            .retain(|id, _| Self::identity(Self::handle(id)).as_deref() == Some(id));
        if windows.is_empty() && !unreadable.is_empty() {
            return Err(format!(
                "无法确认 {matcher} 是否已打开：{} 个候选窗口的进程信息无法读取。请检查应用运行权限 / Cannot safely determine whether the application is running",
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
    fn hide_quake(&mut self, id: &str) -> Result<(), String> {
        let hwnd = self.validate(id)?;
        // Remember before posting: the target can acknowledge after a timeout,
        // and reset/exit must still recover a window hidden by that request.
        self.quake_hidden.insert(id.to_owned());
        if unsafe { IsWindowVisible(hwnd) == 0 } {
            return Ok(());
        }
        if unsafe { ShowWindowAsync(hwnd, SW_HIDE) } == 0 {
            return Err(format!(
                "ShowWindowAsync hide: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.wait_for(id, "收起下拉窗口", |h| unsafe {
            IsWindowVisible(h) == 0
        })
    }
    fn check_quake(&mut self, _config: &QuakeConfig) -> Result<(), String> {
        self.check_available()
    }
    fn quake_work_area(&mut self, monitor: QuakeMonitor) -> Result<WindowRect, String> {
        // Capture on the worker before restore/activation changes foreground.
        let handle = unsafe {
            match monitor {
                QuakeMonitor::Cursor => {
                    let mut point = POINT::default();
                    if GetCursorPos(&mut point) == 0 {
                        return Err(format!("GetCursorPos: {}", std::io::Error::last_os_error()));
                    }
                    MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST)
                }
                QuakeMonitor::Foreground => {
                    MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY)
                }
                QuakeMonitor::Primary => {
                    MonitorFromWindow(std::ptr::null_mut(), MONITOR_DEFAULTTOPRIMARY)
                }
            }
        };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if handle.is_null() || unsafe { GetMonitorInfoW(handle, &mut info) } == 0 {
            return Err(format!(
                "GetMonitorInfoW: {}",
                std::io::Error::last_os_error()
            ));
        }
        let width = u32::try_from(i64::from(info.rcWork.right) - i64::from(info.rcWork.left))
            .map_err(|_| "Invalid Windows monitor work area")?;
        let height = u32::try_from(i64::from(info.rcWork.bottom) - i64::from(info.rcWork.top))
            .map_err(|_| "Invalid Windows monitor work area")?;
        Ok(WindowRect {
            x: info.rcWork.left,
            y: info.rcWork.top,
            width,
            height,
        })
    }
    fn place_quake(
        &mut self,
        id: &str,
        bounds: WindowRect,
        always_on_top: bool,
    ) -> Result<(), String> {
        let hwnd = self.validate(id)?;
        if unsafe { IsWindowVisible(hwnd) == 0 } {
            // A dedicated class can recover a terminal hidden by a previous
            // host. Keep it recoverable even if placement subsequently fails.
            self.quake_hidden.insert(id.to_owned());
        }
        // Restoration must remove maximization before applying the requested
        // work-area fraction. Do not activate it until the engine's focus guard
        // has run again; ordinary (non-Quake) restore behavior is separate.
        if unsafe { IsIconic(hwnd) != 0 || IsZoomed(hwnd) != 0 } {
            if unsafe { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) } == 0 {
                return Err(format!(
                    "ShowWindowAsync: {}",
                    std::io::Error::last_os_error()
                ));
            }
            self.wait_for(id, "恢复下拉窗口", |h| unsafe {
                IsIconic(h) == 0 && IsZoomed(h) == 0
            })?;
            self.restore_maximized.remove(id);
        }
        // Move first and acknowledge the monitor transition. Windows delivers
        // WM_DPICHANGED on cross-monitor movement; only then set the final size.
        Self::position(self.validate(id)?, bounds, true)?;
        self.wait_for(id, "移动到显示器", |h| {
            Self::at_bounds(h, bounds, true)
        })?;
        Self::position(self.validate(id)?, bounds, false)?;
        self.wait_for(id, "调整下拉尺寸", |h| {
            Self::at_bounds(h, bounds, false)
        })?;
        if always_on_top {
            let hwnd = self.validate(id)?;
            self.quake_topmost
                .entry(id.into())
                .or_insert_with(|| Self::topmost(hwnd));
            if unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                )
            } == 0
            {
                return Err(format!(
                    "SetWindowPos topmost: {}",
                    std::io::Error::last_os_error()
                ));
            }
            self.wait_for(id, "置顶", Self::topmost)?;
        }
        // A normally hidden terminal is positioned while still invisible,
        // avoiding a flash at its previous monitor or dimensions.
        self.show_without_activation(id)
    }
    fn release_quake(&mut self, id: &str) -> Result<(), String> {
        let Some(original) = self.quake_topmost.get(id).copied() else {
            return Ok(());
        };
        let Ok(hwnd) = self.validate(id) else {
            // Never apply a saved style to a recycled or inaccessible handle.
            self.quake_topmost.remove(id);
            return Ok(());
        };
        let level = if original {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        if unsafe {
            SetWindowPos(
                hwnd,
                level,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        } == 0
        {
            return Err(format!(
                "Restore window stacking: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.wait_for(id, "恢复窗口层级", |h| Self::topmost(h) == original)?;
        self.quake_topmost.remove(id);
        Ok(())
    }
    fn clear_quake(&mut self) -> Result<(), String> {
        let ids = self.quake_topmost.keys().cloned().collect::<Vec<_>>();
        let mut errors = ids
            .iter()
            .filter_map(|id| self.release_quake(id).err())
            .collect::<Vec<_>>();
        let hidden = self.quake_hidden.iter().cloned().collect::<Vec<_>>();
        // Try visibility recovery even if a style restoration failed.
        errors.extend(hidden.iter().filter_map(|id| self.restore_hidden(id).err()));
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
    fn is_foreground_family(&mut self, id: &str) -> Result<bool, String> {
        let hwnd = self.validate(id)?;
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return Ok(true);
        }
        Ok(hwnd == foreground
            || unsafe { GetAncestor(hwnd, GA_ROOTOWNER) == GetAncestor(foreground, GA_ROOTOWNER) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::HINSTANCE;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    /// Test-owned native windows only; no terminal or user configuration is used.
    struct NativeWindows {
        name: String,
        class: Vec<u16>,
        instance: HINSTANCE,
        handles: Vec<HWND>,
    }

    impl NativeWindows {
        fn new(prefix: &str) -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let name = format!(
                "{prefix}test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let class = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            // SAFETY: a null module name selects this test executable.
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            assert!(!instance.is_null());
            let definition = WNDCLASSW {
                lpfnWndProc: Some(DefWindowProcW),
                hInstance: instance,
                lpszClassName: class.as_ptr(),
                ..Default::default()
            };
            // SAFETY: definition and its null-terminated class name are valid.
            assert_ne!(unsafe { RegisterClassW(&definition) }, 0);
            Self {
                name,
                class,
                instance,
                handles: Vec::new(),
            }
        }

        fn create(&mut self, owner: HWND, style: u32) -> HWND {
            // No WS_VISIBLE: creation must not show or focus a native window.
            // SAFETY: class remains registered until this fixture is dropped.
            let hwnd = unsafe {
                CreateWindowExW(
                    style,
                    self.class.as_ptr(),
                    std::ptr::null(),
                    WS_OVERLAPPEDWINDOW,
                    80,
                    80,
                    640,
                    360,
                    owner,
                    std::ptr::null_mut(),
                    self.instance,
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null(), "{}", std::io::Error::last_os_error());
            self.handles.push(hwnd);
            hwnd
        }
    }

    impl Drop for NativeWindows {
        fn drop(&mut self) {
            // SAFETY: windows are owned by this test thread. Destroy owned
            // dialogs before owners, then unregister the test's private class.
            unsafe {
                for hwnd in self.handles.iter().rev() {
                    DestroyWindow(*hwnd);
                }
                UnregisterClassW(self.class.as_ptr(), self.instance);
            }
        }
    }

    fn desktop() -> WindowsDesktop {
        // HWND operations are testable without requiring the Explorer shell.
        WindowsDesktop {
            restore_maximized: HashMap::new(),
            quake_topmost: HashMap::new(),
            quake_hidden: HashSet::new(),
        }
    }

    #[test]
    fn dedicated_class_finds_hidden_empty_title_and_excludes_dialogs() {
        let mut native = NativeWindows::new(WEZTERM_QUAKE_CLASS_PREFIX);
        let terminal = native.create(std::ptr::null_mut(), WS_EX_APPWINDOW);
        native.create(terminal, WS_EX_APPWINDOW);
        native.create(std::ptr::null_mut(), WS_EX_TOOLWINDOW);
        let mut ordinary = NativeWindows::new("sean-toolbox-window-");
        let ordinary_hwnd = ordinary.create(std::ptr::null_mut(), WS_EX_APPWINDOW);
        let mut desktop = desktop();
        let matcher = format!("class:{}", native.name);
        let windows = desktop.windows(&matcher).unwrap();
        assert_eq!(windows.len(), 1);
        let id = WindowsDesktop::identity(terminal).unwrap();
        assert_eq!(windows[0].id, id);
        assert!(windows[0].hidden);
        assert_eq!(windows[0].title, native.name);

        // Even a remembered dropdown must stay outside ordinary process matching.
        desktop.hide_quake(&id).unwrap();
        let ordinary_id = WindowsDesktop::identity(ordinary_hwnd).unwrap();
        desktop.hide_quake(&ordinary_id).unwrap();
        let executable = std::env::current_exe().unwrap();
        let windows = desktop.windows(executable.to_str().unwrap()).unwrap();
        assert!(windows.iter().any(|window| window.id == ordinary_id));
        assert!(!windows.iter().any(|window| window.id == id));

        drop(native);
        drop(ordinary);
        desktop.clear_quake().unwrap();
        assert!(desktop.quake_hidden.is_empty());
    }

    #[test]
    fn native_quake_hides_reuses_and_recovers_without_activation() {
        let mut native = NativeWindows::new(WEZTERM_QUAKE_CLASS_PREFIX);
        let hwnd = native.create(std::ptr::null_mut(), WS_EX_APPWINDOW);
        let id = WindowsDesktop::identity(hwnd).unwrap();
        let matcher = format!("class:{}", native.name);
        let mut desktop = desktop();
        let bounds = WindowRect {
            x: 80,
            y: 80,
            width: 640,
            height: 360,
        };
        desktop.place_quake(&id, bounds, true).unwrap();
        assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert!(WindowsDesktop::topmost(hwnd));
        assert_ne!(unsafe { GetForegroundWindow() }, hwnd);

        desktop.hide_quake(&id).unwrap();
        desktop.release_quake(&id).unwrap();
        assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert_eq!(unsafe { IsIconic(hwnd) }, 0);
        assert!(!WindowsDesktop::topmost(hwnd));
        let windows = desktop.windows(&matcher).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, id);
        assert!(windows[0].hidden);

        desktop.place_quake(&id, bounds, true).unwrap();
        assert!(!desktop.quake_hidden.contains(&id));
        desktop.hide_quake(&id).unwrap();
        desktop.clear_quake().unwrap();
        assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0);
        assert_eq!(unsafe { IsIconic(hwnd) }, 0);
        assert!(!WindowsDesktop::topmost(hwnd));
        assert!(desktop.quake_hidden.is_empty());
        assert_ne!(unsafe { GetForegroundWindow() }, hwnd);
    }
}
