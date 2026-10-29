//! X11/EWMH backend. XWayland does not grant authority over native Wayland
//! windows, so a Wayland session is rejected rather than partially advertised.
use crate::backend::{Desktop, Window};
use std::path::Path;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::ErrorKind;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, Window as XWindow,
};
use x11rb::rust_connection::RustConnection;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        _NET_SUPPORTED, _NET_SUPPORTING_WM_CHECK, _NET_CLIENT_LIST,
        _NET_ACTIVE_WINDOW, _NET_WM_NAME, UTF8_STRING, _NET_WM_PID,
        _NET_WM_WINDOW_TYPE, _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_DIALOG, _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN, _NET_WM_STATE_MAXIMIZED_VERT,
        _NET_WM_STATE_MAXIMIZED_HORZ, WM_CHANGE_STATE, WM_STATE,
    }
}

pub(crate) struct X11Desktop {
    connection: RustConnection,
    root: XWindow,
    atoms: Atoms,
}

impl X11Desktop {
    pub(crate) fn new() -> Result<Self, String> {
        if std::env::var("XDG_SESSION_TYPE").is_ok_and(|s| s.eq_ignore_ascii_case("wayland"))
            || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty())
        {
            return Err("Wayland 暂不支持控制其他应用窗口；请使用 X11 会话 / Native Wayland window control is unsupported".into());
        }
        if std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty()) {
            return Err("未发现 X11 桌面会话 / DISPLAY is not set".into());
        }
        let (connection, screen) =
            x11rb::connect(None).map_err(|e| format!("Cannot connect to X11: {e}"))?;
        let root = connection.setup().roots[screen].root;
        let atoms = Atoms::new(&connection)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let mut desktop = Self {
            connection,
            root,
            atoms,
        };
        desktop.check_available()?;
        Ok(desktop)
    }

    fn property(
        &self,
        window: XWindow,
        atom: Atom,
        count: u32,
    ) -> Result<x11rb::protocol::xproto::GetPropertyReply, String> {
        self.connection
            .get_property(false, window, atom, AtomEnum::ANY, 0, count)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())
    }
    fn numbers(&self, window: XWindow, atom: Atom) -> Result<Vec<u32>, String> {
        let value = self.property(window, atom, 16384)?;
        Ok(value.value32().map(Iterator::collect).unwrap_or_default())
    }
    fn active(&self) -> Result<XWindow, String> {
        Ok(self
            .numbers(self.root, self.atoms._NET_ACTIVE_WINDOW)?
            .first()
            .copied()
            .unwrap_or(0))
    }
    fn classes(&self, window: XWindow) -> Result<Vec<String>, String> {
        let reply = self.property(window, AtomEnum::WM_CLASS.into(), 4096)?;
        Ok(reply
            .value
            .split(|b| *b == 0)
            .filter(|v| !v.is_empty())
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .collect())
    }
    fn identity(&self, window: XWindow) -> Result<(String, Vec<String>), String> {
        self.connection
            .get_window_attributes(window)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let pid = self
            .numbers(window, self.atoms._NET_WM_PID)?
            .first()
            .copied()
            .unwrap_or(0);
        let mut aliases = self.classes(window)?;
        // Incorporating the process start time catches both a recycled XID and
        // a recycled PID. WM_CLASS remains useful for clients without _NET_WM_PID.
        let start = if pid != 0 {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| {
                    s.rsplit_once(')')
                        .map(|(_, rest)| rest.split_whitespace().nth(19).unwrap_or("0").to_owned())
                })
                .unwrap_or_else(|| "0".into())
        } else {
            "0".into()
        };
        let class_hash = aliases
            .iter()
            .flat_map(|s| s.bytes().chain(std::iter::once(0)))
            .fold(0xcbf29ce484222325u64, |h, c| {
                (h ^ u64::from(c)).wrapping_mul(0x100000001b3)
            });
        if pid != 0
            && let Ok(executable) = std::fs::read_link(format!("/proc/{pid}/exe"))
        {
            aliases.push(executable.to_string_lossy().into_owned());
            if let Some(name) = executable.file_name() {
                aliases.push(name.to_string_lossy().into_owned());
            }
        }
        Ok((format!("x11:{window}:{pid}:{start}:{class_hash}"), aliases))
    }
    fn inspect(&self, window: XWindow) -> Result<(Window, Vec<String>), String> {
        let (id, aliases) = self.identity(window)?;
        let utf8 = self.property(window, self.atoms._NET_WM_NAME, 4096)?.value;
        let title = if utf8.is_empty() {
            self.property(window, AtomEnum::WM_NAME.into(), 4096)?.value
        } else {
            utf8
        };
        let states = self.numbers(window, self.atoms._NET_WM_STATE)?;
        let iconic = self.numbers(window, self.atoms.WM_STATE)?.first() == Some(&3);
        Ok((
            Window {
                id,
                title: String::from_utf8_lossy(&title)
                    .trim_end_matches('\0')
                    .into(),
                minimized: iconic || states.contains(&self.atoms._NET_WM_STATE_HIDDEN),
                maximized: states.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_VERT)
                    && states.contains(&self.atoms._NET_WM_STATE_MAXIMIZED_HORZ),
            },
            aliases,
        ))
    }
    fn validate(&self, id: &str) -> Result<XWindow, String> {
        let window: u32 = id
            .strip_prefix("x11:")
            .and_then(|s| s.split(':').next())
            .and_then(|s| s.parse().ok())
            .ok_or("Invalid X11 window identifier")?;
        if self.identity(window)?.0 != id {
            return Err(
                "Window no longer exists or identity changed / 窗口已关闭或句柄已变化".into(),
            );
        }
        Ok(window)
    }
    fn send(&self, window: XWindow, message: Atom, data: [u32; 5]) -> Result<(), String> {
        let event = ClientMessageEvent::new(32, window, message, data);
        self.connection
            .send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        self.connection.flush().map_err(|e| e.to_string())
    }
    fn wait_for(
        &self,
        id: &str,
        description: &str,
        predicate: impl Fn(&Window) -> bool,
    ) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let window = self.validate(id)?;
            if predicate(&self.inspect(window)?.0) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "The X11 window manager did not acknowledge {description} / 窗口管理器未响应"
                ));
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

impl Desktop for X11Desktop {
    fn check_available(&mut self) -> Result<(), String> {
        let wm = self
            .numbers(self.root, self.atoms._NET_SUPPORTING_WM_CHECK)?
            .first()
            .copied()
            .unwrap_or(0);
        if wm == 0
            || self
                .numbers(wm, self.atoms._NET_SUPPORTING_WM_CHECK)?
                .first()
                != Some(&wm)
        {
            return Err(
                "需要支持 EWMH 的 X11 窗口管理器 / An EWMH-compatible window manager is required"
                    .into(),
            );
        }
        let supported = self.numbers(self.root, self.atoms._NET_SUPPORTED)?;
        if ![
            self.atoms._NET_CLIENT_LIST,
            self.atoms._NET_ACTIVE_WINDOW,
            self.atoms._NET_WM_STATE,
        ]
        .iter()
        .all(|a| supported.contains(a))
        {
            return Err(
                "当前 X11 窗口管理器缺少 EWMH 窗口控制能力 / Missing EWMH capabilities".into(),
            );
        }
        Ok(())
    }
    fn windows(&mut self, matcher: &str) -> Result<Vec<Window>, String> {
        self.check_available()?;
        let mut windows = Vec::new();
        for window in self.numbers(self.root, self.atoms._NET_CLIENT_LIST)? {
            let inspection = (|| {
                let types = self.numbers(window, self.atoms._NET_WM_WINDOW_TYPE)?;
                if !types.is_empty()
                    && !types.iter().any(|a| {
                        *a == self.atoms._NET_WM_WINDOW_TYPE_NORMAL
                            || *a == self.atoms._NET_WM_WINDOW_TYPE_DIALOG
                    })
                {
                    return Ok(None);
                }
                let (info, aliases) = self.inspect(window)?;
                Ok(Some((info, aliases)))
            })();
            match inspection {
                Ok(Some((info, aliases))) => {
                    let basename = Path::new(matcher)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or(matcher);
                    if aliases.iter().any(|alias| {
                        alias.eq_ignore_ascii_case(matcher) || alias.eq_ignore_ascii_case(basename)
                    }) {
                        windows.push(info);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    // A client can disappear between reading the root list and
                    // reading its properties. Other failures must remain errors
                    // rather than masquerading as an empty list and launching duplicates.
                    if !matches!(self
                        .connection
                        .get_window_attributes(window)
                        .map_err(|e| e.to_string())?
                        .reply(), Err(ReplyError::X11Error(e)) if e.error_kind == ErrorKind::Window)
                    {
                        return Err(error);
                    }
                }
            }
        }
        Ok(windows)
    }
    fn foreground(&mut self) -> Result<Option<String>, String> {
        for attempt in 0..4 {
            let window = self.active()?;
            if window == 0 {
                return Ok(None);
            }
            match self.identity(window) {
                Ok((id, _)) => return Ok(Some(id)),
                Err(error) => {
                    let gone = matches!(self.connection.get_window_attributes(window).map_err(|e| e.to_string())?.reply(), Err(ReplyError::X11Error(e)) if e.error_kind == ErrorKind::Window);
                    if !gone {
                        return Err(error);
                    }
                    // A destroyed active client may remain in the EWMH root
                    // property for one WM event-loop iteration. Retry briefly;
                    // if it persists, preserve an unknown-focus marker so a
                    // subsequent launch will not treat it as an empty desktop.
                    if attempt == 3 {
                        return Ok(Some(format!("x11-gone:{window}")));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        unreachable!("all retry paths return on the final attempt")
    }
    fn minimize(&mut self, id: &str) -> Result<(), String> {
        let window = self.validate(id)?;
        self.send(window, self.atoms.WM_CHANGE_STATE, [3, 0, 0, 0, 0])?;
        self.wait_for(id, "minimize", |w| w.minimized)
    }
    fn restore(&mut self, id: &str) -> Result<(), String> {
        let window = self.validate(id)?;
        self.connection
            .map_window(window)
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        self.connection.flush().map_err(|e| e.to_string())?;
        self.wait_for(id, "restore", |w| !w.minimized)
    }
    fn maximize(&mut self, id: &str) -> Result<(), String> {
        let window = self.validate(id)?;
        self.send(
            window,
            self.atoms._NET_WM_STATE,
            [
                1,
                self.atoms._NET_WM_STATE_MAXIMIZED_HORZ,
                self.atoms._NET_WM_STATE_MAXIMIZED_VERT,
                2,
                0,
            ],
        )?;
        self.wait_for(id, "maximize", |w| w.maximized)
    }
    fn activate(&mut self, id: &str) -> Result<bool, String> {
        let window = self.validate(id)?;
        let original = self.active()?;
        if original == window {
            return Ok(true);
        }
        self.send(
            window,
            self.atoms._NET_ACTIVE_WINDOW,
            [2, x11rb::CURRENT_TIME, original, 0, 0],
        )?;
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            let active = self.active()?;
            if active == window {
                return Ok(true);
            }
            if (active != 0 && active != original) || Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::{Engine, EngineOptions};
    use std::process::{Child, Command, Stdio};
    use toolbox_core::{AppProfile, LaunchCommand, Platform};

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Run inside an isolated Xvfb + EWMH window manager. This test deliberately
    /// changes window focus; it is not part of the default headless test suite.
    #[test]
    #[ignore = "requires an isolated X11 session, openbox and xterm"]
    fn native_x11_smoke() {
        assert_eq!(
            std::env::var("TOOLBOX_X11_SMOKE").as_deref(),
            Ok("1"),
            "Use the repository's Xvfb smoke script; do not run on a personal desktop"
        );
        let mut desktop = X11Desktop::new().expect("X11 / EWMH backend");
        let class = format!("ToolboxSmoke{}", std::process::id());
        let mut children = Vec::new();
        for n in 1..=2 {
            children.push(ChildGuard(
                Command::new("xterm")
                    .args(["-class", &class, "-title", &format!("Toolbox smoke {n}")])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let windows = loop {
            let windows = desktop.windows(&class).unwrap();
            if windows.len() == 2 {
                break windows;
            }
            assert!(Instant::now() < deadline, "xterm did not create windows");
            std::thread::sleep(Duration::from_millis(30));
        };
        assert!(desktop.activate(&windows[0].id).unwrap());
        desktop.maximize(&windows[0].id).unwrap();
        desktop.minimize(&windows[0].id).unwrap();
        desktop.restore(&windows[0].id).unwrap();
        assert!(
            desktop
                .windows(&class)
                .unwrap()
                .iter()
                .find(|w| w.id == windows[0].id)
                .unwrap()
                .maximized,
            "restoring must preserve maximization"
        );
        assert!(desktop.activate(&windows[0].id).unwrap());
        let profile = AppProfile {
            id: "smoke".into(),
            name: "Smoke".into(),
            platform: Platform::Linux,
            matcher: class.clone(),
            hotkey: String::new(),
            enabled: true,
            maximize: false,
            cycle_continuous: false,
            launch: vec![LaunchCommand {
                executable: "xterm".into(),
                args: vec![],
                cwd: None,
            }],
        };
        let mut engine = Engine::with_options(desktop, EngineOptions::default());
        engine.toggle(&profile).unwrap();
        assert_eq!(
            engine.backend_mut().foreground().unwrap(),
            Some(windows[1].id.clone())
        );
        engine.toggle(&profile).unwrap();
        assert!(
            engine
                .backend_mut()
                .windows(&class)
                .unwrap()
                .iter()
                .all(|w| w.minimized)
        );
        engine.toggle(&profile).unwrap();
        assert_eq!(
            engine.backend_mut().foreground().unwrap(),
            Some(windows[0].id.clone())
        );
        for child in &mut children {
            child.0.kill().unwrap();
            child.0.wait().unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !engine.backend_mut().windows(&class).unwrap().is_empty() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            engine.backend_mut().activate(&windows[0].id).is_err(),
            "stale window must be rejected"
        );
        // Exercise the real launch path too: this subprocess is reaped by the
        // launch helper; closing its X11 connection terminates the smoke client.
        let mut launched = profile;
        launched.id = "smoke-launch".into();
        launched.matcher = format!("{class}Launch");
        launched.maximize = true;
        launched.launch[0].args = vec![
            "-class".into(),
            launched.matcher.clone(),
            "-title".into(),
            "Toolbox launched smoke".into(),
        ];
        engine.toggle(&launched).unwrap();
        let launched_windows = engine.backend_mut().windows(&launched.matcher).unwrap();
        assert_eq!(launched_windows.len(), 1);
        assert!(
            launched_windows[0].maximized,
            "newly launched window should maximize"
        );
        let backend = engine.backend_mut();
        let xid = backend.validate(&launched_windows[0].id).unwrap();
        backend
            .connection
            .kill_client(xid)
            .unwrap()
            .check()
            .unwrap();
        backend.connection.flush().unwrap();
    }
}
