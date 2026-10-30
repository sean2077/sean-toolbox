//! macOS Accessibility + AppKit FFI. No AppleScript/osascript helper process,
//! JavaScript engine, or keyboard injection is used.
use crate::backend::{Desktop, Window, WindowRect};
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::time::{Duration, Instant};
use toolbox_core::{QuakeConfig, QuakeMonitor};

type Ref = *const c_void;
type Obj = *mut c_void;
const UTF8: u32 = 0x0800_0100;
const AX_NO_VALUE: i32 = -25212;
const AX_UNSUPPORTED: i32 = -25205;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXUIElementCreateApplication(pid: i32) -> Ref;
    fn AXUIElementCopyAttributeValue(element: Ref, name: Ref, value: *mut Ref) -> i32;
    fn AXUIElementSetAttributeValue(element: Ref, name: Ref, value: Ref) -> i32;
    fn AXUIElementIsAttributeSettable(element: Ref, name: Ref, value: *mut u8) -> i32;
    fn AXUIElementPerformAction(element: Ref, action: Ref) -> i32;
    fn AXUIElementGetPid(element: Ref, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: Ref, timeout: f32) -> i32;
    fn AXUIElementGetTypeID() -> usize;
    fn AXValueGetTypeID() -> usize;
    fn AXValueCreate(kind: u32, value: *const c_void) -> Ref;
    fn AXValueGetValue(value: Ref, kind: u32, output: *mut c_void) -> u8;
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(value: Ref) -> Ref;
    fn CFRelease(value: Ref);
    fn CFEqual(a: Ref, b: Ref) -> u8;
    fn CFGetTypeID(value: Ref) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFStringCreateWithCString(allocator: Ref, value: *const c_char, encoding: u32) -> Ref;
    fn CFStringGetLength(value: Ref) -> isize;
    fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFBooleanGetValue(value: Ref) -> u8;
    fn CFArrayGetCount(value: Ref) -> isize;
    fn CFArrayGetValueAtIndex(value: Ref, index: isize) -> Ref;
    static kCFBooleanTrue: Ref;
    static kCFBooleanFalse: Ref;
}
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Obj;
    fn sel_registerName(name: *const c_char) -> Obj;
    fn objc_msgSend();
    #[cfg(target_arch = "x86_64")]
    fn objc_msgSend_stret();
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Point {
    x: f64,
    y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Size {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct Rect {
    origin: Point,
    size: Size,
}

// Each small wrapper casts objc_msgSend to the exact selector signature. The
// caller only supplies the documented AppKit/Foundation selectors below. An
// autorelease pool encloses every public desktop operation on the worker thread.
unsafe fn send_ptr(object: Obj, name: &CStr) -> Obj {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj) -> Obj =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(object, sel_registerName(name.as_ptr()))
    }
}
unsafe fn send_index(object: Obj, name: &CStr, index: usize) -> Obj {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj, usize) -> Obj =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(object, sel_registerName(name.as_ptr()), index)
    }
}
unsafe fn send_integer(object: Obj, name: &CStr) -> isize {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj) -> isize =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(object, sel_registerName(name.as_ptr()))
    }
}
unsafe fn send_pid(object: Obj) -> i32 {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj) -> i32 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(object, sel_registerName(c"processIdentifier".as_ptr()))
    }
}
unsafe fn running_app(pid: i32) -> Obj {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj, i32) -> Obj =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(
            objc_getClass(c"NSRunningApplication".as_ptr()),
            sel_registerName(c"runningApplicationWithProcessIdentifier:".as_ptr()),
            pid,
        )
    }
}
unsafe fn activate_app(app: Obj) -> bool {
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj, usize) -> i8 =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(app, sel_registerName(c"activateWithOptions:".as_ptr()), 0) != 0
    }
}
unsafe fn screen_rect(screen: Obj, selector: &CStr) -> Rect {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        // A 32-byte aggregate uses objc_msgSend_stret on Intel macOS.
        let f: unsafe extern "C" fn(*mut Rect, Obj, Obj) =
            std::mem::transmute(objc_msgSend_stret as unsafe extern "C" fn());
        let mut rect = Rect::default();
        f(&mut rect, screen, sel_registerName(selector.as_ptr()));
        rect
    }
    #[cfg(not(target_arch = "x86_64"))]
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj) -> Rect =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(screen, sel_registerName(selector.as_ptr()))
    }
}

fn cursor_position() -> Option<Point> {
    unsafe {
        let array = send_ptr(objc_getClass(c"NSScreen".as_ptr()), c"screens");
        if send_integer(array, c"count") == 0 {
            return None;
        }
        let primary = screen_rect(send_index(array, c"objectAtIndex:", 0), c"frame");
        let f: unsafe extern "C" fn(Obj, Obj) -> Point =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let mut point = f(
            objc_getClass(c"NSEvent".as_ptr()),
            sel_registerName(c"mouseLocation".as_ptr()),
        );
        point.y = primary.origin.y + primary.size.height - point.y;
        (point.x.is_finite() && point.y.is_finite()).then_some(point)
    }
}

struct Pool(Obj);
impl Pool {
    fn new() -> Self {
        unsafe {
            Self(send_ptr(
                send_ptr(objc_getClass(c"NSAutoreleasePool".as_ptr()), c"alloc"),
                c"init",
            ))
        }
    }
}
impl Drop for Pool {
    fn drop(&mut self) {
        unsafe {
            let f: unsafe extern "C" fn(Obj, Obj) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            f(self.0, sel_registerName(c"drain".as_ptr()));
        }
    }
}

/// Exactly one owned CoreFoundation retain; borrowed values are retained before
/// wrapping, and copy/create functions transfer their own retain into this type.
struct Cf(Ref);
impl Cf {
    fn owned(value: Ref) -> Result<Self, String> {
        if value.is_null() {
            Err("macOS returned a null CoreFoundation object".into())
        } else {
            Ok(Self(value))
        }
    }
    fn attribute(name: &CStr) -> Result<Self, String> {
        Self::owned(unsafe { CFStringCreateWithCString(ptr::null(), name.as_ptr(), UTF8) })
    }
    fn is_type(&self, kind: usize) -> bool {
        unsafe { CFGetTypeID(self.0) == kind }
    }
}
impl Clone for Cf {
    fn clone(&self) -> Self {
        Self(unsafe { CFRetain(self.0) })
    }
}
impl Drop for Cf {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}

fn copy(element: Ref, name: &CStr, optional: bool) -> Result<Option<Cf>, String> {
    let attribute = Cf::attribute(name)?;
    let mut value = ptr::null();
    let status = unsafe { AXUIElementCopyAttributeValue(element, attribute.0, &mut value) };
    if status != 0 {
        if !value.is_null() {
            unsafe {
                CFRelease(value);
            }
        }
        if status == AX_NO_VALUE || (optional && status == AX_UNSUPPORTED) {
            return Ok(None);
        }
        return Err(format!(
            "无法读取 {}（AX {status}）；请检查辅助功能权限和应用响应 / Accessibility query failed",
            name.to_string_lossy()
        ));
    }
    Ok(Some(Cf::owned(value)?))
}
fn app_element(pid: i32) -> Result<Cf, String> {
    let app = Cf::owned(unsafe { AXUIElementCreateApplication(pid) })?;
    unsafe {
        AXUIElementSetMessagingTimeout(app.0, 0.2);
    }
    Ok(app)
}
fn text(value: &Cf) -> String {
    if !value.is_type(unsafe { CFStringGetTypeID() }) {
        return String::new();
    }
    let count = (unsafe { CFStringGetLength(value.0) }.max(0) as usize * 4 + 1).clamp(4, 1_048_576);
    let mut buffer = vec![0u8; count];
    if unsafe { CFStringGetCString(value.0, buffer.as_mut_ptr().cast(), count as isize, UTF8) } == 0
    {
        return String::new();
    }
    let length = buffer.iter().position(|b| *b == 0).unwrap_or(count);
    String::from_utf8_lossy(&buffer[..length]).into_owned()
}
unsafe fn ns_text(value: Obj) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe {
        let f: unsafe extern "C" fn(Obj, Obj) -> *const c_char =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let ptr = f(value, sel_registerName(c"UTF8String".as_ptr()));
        if ptr.is_null() {
            String::new()
        } else {
            CStr::from_ptr(ptr).to_string_lossy().into_owned()
        }
    }
}
fn boolean(window: Ref, name: &CStr) -> Result<bool, String> {
    Ok(copy(window, name, true)?.is_some_and(|v| {
        v.is_type(unsafe { CFBooleanGetTypeID() }) && unsafe { CFBooleanGetValue(v.0) != 0 }
    }))
}
fn require_settable(window: Ref, name: &CStr) -> Result<(), String> {
    let attribute = Cf::attribute(name)?;
    let mut writable = 0;
    let status = unsafe { AXUIElementIsAttributeSettable(window, attribute.0, &mut writable) };
    if status == 0 && writable != 0 {
        Ok(())
    } else {
        Err(format!(
            "应用不允许修改 {}（AX {status}） / This window cannot be modified",
            name.to_string_lossy()
        ))
    }
}
fn set(window: Ref, name: &CStr, value: Ref) -> Result<(), String> {
    let name = Cf::attribute(name)?;
    let status = unsafe { AXUIElementSetAttributeValue(window, name.0, value) };
    if status == 0 {
        Ok(())
    } else {
        Err(format!(
            "macOS 窗口操作失败（AX {status}） / Accessibility mutation failed"
        ))
    }
}
fn frame(window: Ref) -> Result<Option<Rect>, String> {
    let (Some(position), Some(size)) = (
        copy(window, c"AXPosition", true)?,
        copy(window, c"AXSize", true)?,
    ) else {
        return Ok(None);
    };
    if !position.is_type(unsafe { AXValueGetTypeID() })
        || !size.is_type(unsafe { AXValueGetTypeID() })
    {
        return Ok(None);
    }
    let mut rect = Rect::default();
    let valid = unsafe {
        AXValueGetValue(position.0, 1, (&mut rect.origin as *mut Point).cast()) != 0
            && AXValueGetValue(size.0, 2, (&mut rect.size as *mut Size).cast()) != 0
    };
    Ok((valid && valid_rect(rect)).then_some(rect))
}
fn valid_rect(rect: Rect) -> bool {
    [
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    ]
    .iter()
    .all(|v| v.is_finite())
        && rect.size.width > 0.0
        && rect.size.height > 0.0
}
fn same_frame(a: Rect, b: Rect) -> bool {
    (a.origin.x - b.origin.x).abs() <= 4.0
        && (a.origin.y - b.origin.y).abs() <= 4.0
        && (a.size.width - b.size.width).abs() <= 4.0
        && (a.size.height - b.size.height).abs() <= 4.0
}
fn screens() -> Vec<Rect> {
    unsafe {
        let array = send_ptr(objc_getClass(c"NSScreen".as_ptr()), c"screens");
        let count = send_integer(array, c"count").clamp(0, 64) as usize;
        if count == 0 {
            return Vec::new();
        }
        let primary = screen_rect(send_index(array, c"objectAtIndex:", 0), c"frame");
        let top = primary.origin.y + primary.size.height;
        (0..count)
            .filter_map(|i| {
                let mut frame =
                    screen_rect(send_index(array, c"objectAtIndex:", i), c"visibleFrame");
                // Cocoa bottom-left -> Accessibility primary-display top-left.
                frame.origin.y = top - frame.origin.y - frame.size.height;
                valid_rect(frame).then_some(frame)
            })
            .collect()
    }
}
fn target_screen(frame: Option<Rect>, screens: &[Rect]) -> Option<Rect> {
    let Some(frame) = frame else {
        return screens.first().copied();
    };
    let area = |screen: &Rect| {
        (f64::min(
            frame.origin.x + frame.size.width,
            screen.origin.x + screen.size.width,
        ) - f64::max(frame.origin.x, screen.origin.x))
        .max(0.0)
            * (f64::min(
                frame.origin.y + frame.size.height,
                screen.origin.y + screen.size.height,
            ) - f64::max(frame.origin.y, screen.origin.y))
            .max(0.0)
    };
    let distance = |screen: &Rect| {
        let x = frame.origin.x + frame.size.width / 2.0;
        let y = frame.origin.y + frame.size.height / 2.0;
        (x - x.clamp(screen.origin.x, screen.origin.x + screen.size.width)).powi(2)
            + (y - y.clamp(screen.origin.y, screen.origin.y + screen.size.height)).powi(2)
    };
    // Prefer greatest overlap, then nearest work area for cursor positions in
    // menu bars, docks or display gaps. Equal candidates keep primary order.
    screens.iter().copied().reduce(|best, candidate| {
        if area(&candidate) > area(&best)
            || (area(&candidate) == area(&best) && distance(&candidate) < distance(&best))
        {
            candidate
        } else {
            best
        }
    })
}

#[derive(Clone)]
struct App {
    pid: i32,
    identity: String,
    aliases: Vec<String>,
}
struct Entry {
    window: Cf,
    app: App,
    maximized: bool,
}
pub(crate) struct MacDesktop {
    entries: HashMap<String, Entry>,
    sequence: u64,
}

impl MacDesktop {
    pub(crate) fn new() -> Result<Self, String> {
        let mut desktop = Self {
            entries: HashMap::new(),
            sequence: 0,
        };
        desktop.check_available()?;
        Ok(desktop)
    }
    fn workspace() -> Result<Obj, String> {
        let workspace =
            unsafe { send_ptr(objc_getClass(c"NSWorkspace".as_ptr()), c"sharedWorkspace") };
        if workspace.is_null() {
            Err("No macOS desktop workspace is available / 未发现 macOS 桌面会话".into())
        } else {
            Ok(workspace)
        }
    }
    fn describe(app: Obj) -> Option<App> {
        if app.is_null() {
            return None;
        }
        unsafe {
            if !matches!(send_integer(app, c"activationPolicy"), 0 | 1) {
                return None;
            }
            let pid = send_pid(app);
            if pid <= 0 {
                return None;
            }
            let bundle = ns_text(send_ptr(app, c"bundleIdentifier"));
            let name = ns_text(send_ptr(app, c"localizedName"));
            let executable = ns_text(send_ptr(send_ptr(app, c"executableURL"), c"path"));
            let basename = std::path::Path::new(&executable)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_owned();
            let identity = if bundle.is_empty() {
                executable.clone()
            } else {
                bundle.clone()
            };
            let aliases = [bundle, name, executable, basename]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            Some(App {
                pid,
                identity,
                aliases,
            })
        }
    }
    fn apps() -> Result<Vec<App>, String> {
        unsafe {
            let array = send_ptr(Self::workspace()?, c"runningApplications");
            let count = send_integer(array, c"count").clamp(0, 10000) as usize;
            Ok((0..count)
                .filter_map(|i| Self::describe(send_index(array, c"objectAtIndex:", i)))
                .collect())
        }
    }
    fn copy_windows(pid: i32) -> Result<Vec<Cf>, String> {
        let app = app_element(pid)?;
        let Some(array) = copy(app.0, c"AXWindows", false)? else {
            return Ok(Vec::new());
        };
        if !array.is_type(unsafe { CFArrayGetTypeID() }) {
            return Err("Invalid AXWindows list".into());
        }
        let count = unsafe { CFArrayGetCount(array.0) }.clamp(0, 10000);
        let mut windows = Vec::new();
        for i in 0..count {
            let window = unsafe { CFArrayGetValueAtIndex(array.0, i) };
            if !window.is_null() && unsafe { CFGetTypeID(window) == AXUIElementGetTypeID() } {
                let window = Cf::owned(unsafe { CFRetain(window) })?;
                unsafe {
                    AXUIElementSetMessagingTimeout(window.0, 0.2);
                }
                windows.push(window);
            }
        }
        Ok(windows)
    }
    fn register(&mut self, window: &Cf, app: &App) -> String {
        if let Some((id, _)) = self.entries.iter().find(|(_, e)| {
            e.app.pid == app.pid
                && e.app.identity == app.identity
                && unsafe { CFEqual(e.window.0, window.0) != 0 }
        }) {
            return id.clone();
        }
        self.sequence += 1;
        let id = format!("ax:{}:{}", app.pid, self.sequence);
        self.entries.insert(
            id.clone(),
            Entry {
                window: window.clone(),
                app: app.clone(),
                maximized: false,
            },
        );
        id
    }
    fn info(&mut self, window: &Cf, app: &App, screens: &[Rect]) -> Result<Option<Window>, String> {
        let mut pid = 0;
        if unsafe { AXUIElementGetPid(window.0, &mut pid) } != 0 || pid != app.pid {
            return Ok(None);
        }
        let role = copy(window.0, c"AXRole", false)?.ok_or("Cannot read AX window role")?;
        if text(&role) != "AXWindow" {
            return Ok(None);
        }
        let title = copy(window.0, c"AXTitle", true)?
            .as_ref()
            .map(text)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| app.aliases.first().cloned().unwrap_or_default());
        let minimized = boolean(window.0, c"AXMinimized")?;
        let id = self.register(window, app);
        let entry = self.entries.get_mut(&id).expect("registered above");
        if !minimized {
            entry.maximized = boolean(window.0, c"AXFullScreen")?
                || frame(window.0)?.is_some_and(|r| screens.iter().any(|s| same_frame(r, *s)));
        }
        Ok(Some(Window {
            id,
            title,
            minimized,
            maximized: entry.maximized,
        }))
    }
    fn validate(&mut self, id: &str) -> Result<(Cf, App), String> {
        self.check_available()?;
        let entry = self
            .entries
            .get(id)
            .ok_or("Unknown or expired macOS window token")?;
        let app = Self::apps()?
            .into_iter()
            .find(|a| a.pid == entry.app.pid && a.identity == entry.app.identity)
            .ok_or("The macOS application has exited or was replaced")?;
        if !Self::copy_windows(app.pid)?
            .iter()
            .any(|w| unsafe { CFEqual(w.0, entry.window.0) != 0 })
        {
            self.entries.remove(id);
            return Err("窗口已关闭或被替换 / The macOS window no longer exists".into());
        }
        Ok((entry.window.clone(), app))
    }
    fn wait_for(&mut self, id: &str, expected: bool) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(800);
        loop {
            let (window, _) = self.validate(id)?;
            if boolean(window.0, c"AXMinimized")? == expected {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "macOS 窗口未响应最小化/恢复操作 / Window did not acknowledge minimize/restore"
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

impl Desktop for MacDesktop {
    fn check_available(&mut self) -> Result<(), String> {
        if unsafe { AXIsProcessTrusted() } == 0 {
            Err("需要辅助功能权限：系统设置 → 隐私与安全性 → 辅助功能，授权 Sean Toolbox 的后台进程，然后重试 / Accessibility permission is required for the host executable".into())
        } else {
            Ok(())
        }
    }
    fn windows(&mut self, matcher: &str) -> Result<Vec<Window>, String> {
        let _pool = Pool::new();
        self.check_available()?;
        let apps = Self::apps()?;
        let pids: HashSet<_> = apps.iter().map(|a| a.pid).collect();
        self.entries
            .retain(|_, entry| pids.contains(&entry.app.pid));
        let screens = screens();
        let mut result = Vec::new();
        for app in apps
            .into_iter()
            .filter(|a| a.aliases.iter().any(|v| v.eq_ignore_ascii_case(matcher)))
        {
            let windows = Self::copy_windows(app.pid)?;
            self.entries.retain(|_, entry| {
                entry.app.pid != app.pid
                    || windows
                        .iter()
                        .any(|w| unsafe { CFEqual(w.0, entry.window.0) != 0 })
            });
            for window in windows {
                if let Some(info) = self.info(&window, &app, &screens)? {
                    result.push(info);
                }
            }
        }
        Ok(result)
    }
    fn foreground(&mut self) -> Result<Option<String>, String> {
        let _pool = Pool::new();
        self.check_available()?;
        let object = unsafe { send_ptr(Self::workspace()?, c"frontmostApplication") };
        let Some(app) = Self::describe(object) else {
            return Ok(None);
        };
        let element = app_element(app.pid)?;
        let Some(window) = copy(element.0, c"AXFocusedWindow", true)? else {
            return Ok(Some(format!("mac-app:{}", app.pid)));
        };
        if !window.is_type(unsafe { AXUIElementGetTypeID() }) {
            return Err("Invalid AXFocusedWindow value".into());
        }
        Ok(Some(self.register(&window, &app)))
    }
    fn minimize(&mut self, id: &str) -> Result<(), String> {
        let _pool = Pool::new();
        let (window, _) = self.validate(id)?;
        require_settable(window.0, c"AXMinimized")?;
        set(window.0, c"AXMinimized", unsafe { kCFBooleanTrue })?;
        self.wait_for(id, true)
    }
    fn restore(&mut self, id: &str) -> Result<(), String> {
        let _pool = Pool::new();
        let (window, _) = self.validate(id)?;
        if !boolean(window.0, c"AXMinimized")? {
            return Ok(());
        }
        require_settable(window.0, c"AXMinimized")?;
        set(window.0, c"AXMinimized", unsafe { kCFBooleanFalse })?;
        self.wait_for(id, false)
    }
    fn maximize(&mut self, id: &str) -> Result<(), String> {
        let _pool = Pool::new();
        let (window, _) = self.validate(id)?;
        if boolean(window.0, c"AXFullScreen")? {
            return Ok(());
        }
        let target = target_screen(frame(window.0)?, &screens())
            .ok_or("No macOS display work area is available")?;
        require_settable(window.0, c"AXPosition")?;
        require_settable(window.0, c"AXSize")?;
        let position =
            Cf::owned(unsafe { AXValueCreate(1, (&target.origin as *const Point).cast()) })?;
        let size = Cf::owned(unsafe { AXValueCreate(2, (&target.size as *const Size).cast()) })?;
        set(window.0, c"AXPosition", position.0)?;
        set(window.0, c"AXSize", size.0)?;
        set(window.0, c"AXPosition", position.0)?;
        // Some applications constrain their minimum/maximum dimensions. Report
        // the real result rather than marking an unacknowledged resize as done.
        let deadline = Instant::now() + Duration::from_millis(800);
        loop {
            let (window, _) = self.validate(id)?;
            if frame(window.0)?.is_some_and(|r| same_frame(r, target)) {
                if let Some(entry) = self.entries.get_mut(id) {
                    entry.maximized = true;
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "应用未接受显示器工作区尺寸 / This application did not accept maximized bounds"
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    fn activate(&mut self, id: &str) -> Result<bool, String> {
        let _pool = Pool::new();
        let (window, app) = self.validate(id)?;
        let original = self.foreground()?;
        if original.as_deref() == Some(id) {
            return Ok(true);
        }
        let running = unsafe { running_app(app.pid) };
        if running.is_null() {
            return Err("macOS application has exited".into());
        }
        if !unsafe { activate_app(running) } {
            return Ok(false);
        }
        let target_prefix = format!("ax:{}:", app.pid);
        let target_app = format!("mac-app:{}", app.pid);
        let unchanged = |focus: &Option<String>| {
            focus == &original
                || focus
                    .as_ref()
                    .is_some_and(|f| f.starts_with(&target_prefix) || f == &target_app)
        };
        if !unchanged(&self.foreground()?) {
            return Ok(false);
        }
        let application = app_element(app.pid)?;
        if require_settable(application.0, c"AXFocusedWindow").is_ok() {
            set(application.0, c"AXFocusedWindow", window.0)?;
        }
        if require_settable(window.0, c"AXMain").is_ok() {
            if !unchanged(&self.foreground()?) {
                return Ok(false);
            }
            set(window.0, c"AXMain", unsafe { kCFBooleanTrue })?;
        }
        if !unchanged(&self.foreground()?) {
            return Ok(false);
        }
        let action = Cf::attribute(c"AXRaise")?;
        let status = unsafe { AXUIElementPerformAction(window.0, action.0) };
        if status != 0 && status != -25204 {
            return Err(format!("AXRaise failed ({status})"));
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let now = self.foreground()?;
            if now.as_deref() == Some(id) {
                return Ok(true);
            }
            if !unchanged(&now) || Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    fn check_quake(&mut self, config: &QuakeConfig) -> Result<(), String> {
        self.check_available()?;
        if config.always_on_top {
            return Err("macOS 不支持为其他应用设置置顶，请关闭此选项 / macOS cannot set always-on-top for another application".into());
        }
        if config.hide_on_focus_loss {
            return Err("失焦自动收起目前仅支持 Windows / Hide on focus loss is currently supported on Windows only".into());
        }
        Ok(())
    }
    fn quake_work_area(&mut self, monitor: QuakeMonitor) -> Result<WindowRect, String> {
        let _pool = Pool::new();
        let screens = screens();
        let reference = match monitor {
            QuakeMonitor::Primary => None,
            QuakeMonitor::Cursor => cursor_position().map(|origin| Rect {
                origin,
                size: Size {
                    width: 1.0,
                    height: 1.0,
                },
            }),
            QuakeMonitor::Foreground => {
                let id = self.foreground()?;
                match id.and_then(|id| self.entries.get(&id)) {
                    Some(entry) => frame(entry.window.0)?,
                    None => None,
                }
            }
        };
        let target =
            target_screen(reference, &screens).ok_or("No macOS display work area is available")?;
        Ok(WindowRect {
            x: target.origin.x.round() as i32,
            y: target.origin.y.round() as i32,
            width: target.size.width.round() as u32,
            height: target.size.height.round() as u32,
        })
    }
    fn place_quake(
        &mut self,
        id: &str,
        bounds: WindowRect,
        always_on_top: bool,
    ) -> Result<(), String> {
        let _pool = Pool::new();
        if always_on_top {
            return Err("macOS cannot set always-on-top for another application / macOS 不支持为其他应用设置置顶".into());
        }
        let (window, _) = self.validate(id)?;
        if boolean(window.0, c"AXFullScreen")? {
            return Err("请先退出应用全屏，再启用下拉窗口 / Leave application fullscreen before using Quake docking".into());
        }
        require_settable(window.0, c"AXPosition")?;
        require_settable(window.0, c"AXSize")?;
        self.restore(id)?;
        let target = Rect {
            origin: Point {
                x: f64::from(bounds.x),
                y: f64::from(bounds.y),
            },
            size: Size {
                width: f64::from(bounds.width),
                height: f64::from(bounds.height),
            },
        };
        let position =
            Cf::owned(unsafe { AXValueCreate(1, (&target.origin as *const Point).cast()) })?;
        let size = Cf::owned(unsafe { AXValueCreate(2, (&target.size as *const Size).cast()) })?;
        set(window.0, c"AXPosition", position.0)?;
        set(window.0, c"AXSize", size.0)?;
        set(window.0, c"AXPosition", position.0)?;
        let deadline = Instant::now() + Duration::from_millis(800);
        loop {
            let (window, _) = self.validate(id)?;
            if frame(window.0)?.is_some_and(|r| same_frame(r, target)) {
                if let Some(entry) = self.entries.get_mut(id) {
                    entry.maximized = false;
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "应用未接受下拉窗口尺寸 / This application did not accept Quake bounds".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn work_area_selection_uses_overlap_and_handles_negative_monitors() {
        let left = Rect {
            origin: Point { x: -1920.0, y: 0.0 },
            size: Size {
                width: 1920.0,
                height: 1040.0,
            },
        };
        let primary = Rect {
            origin: Point { x: 0.0, y: 24.0 },
            size: Size {
                width: 2560.0,
                height: 1376.0,
            },
        };
        let window = Rect {
            origin: Point {
                x: -1000.0,
                y: 100.0,
            },
            size: Size {
                width: 800.0,
                height: 600.0,
            },
        };
        assert!(same_frame(
            target_screen(Some(window), &[primary, left]).unwrap(),
            left
        ));
    }

    #[test]
    fn cursor_in_menu_bar_or_display_gap_uses_nearest_screen() {
        let primary = Rect {
            origin: Point { x: 0.0, y: 24.0 },
            size: Size {
                width: 1600.0,
                height: 876.0,
            },
        };
        let secondary = Rect {
            origin: Point { x: 1600.0, y: 24.0 },
            size: Size {
                width: 1920.0,
                height: 1056.0,
            },
        };
        let point = |x, y| {
            Some(Rect {
                origin: Point { x, y },
                size: Size {
                    width: 1.0,
                    height: 1.0,
                },
            })
        };
        assert!(same_frame(
            target_screen(point(200.0, 8.0), &[primary, secondary]).unwrap(),
            primary
        ));
        assert!(same_frame(
            target_screen(point(2000.0, 8.0), &[primary, secondary]).unwrap(),
            secondary
        ));
        assert!(same_frame(
            target_screen(None, &[primary, secondary]).unwrap(),
            primary
        ));
    }
}
