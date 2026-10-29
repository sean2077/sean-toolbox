use crate::{
    autostart,
    hotkeys::{Binding, Hotkeys},
    ui_process::UiProcess,
};
use global_hotkey::{GlobalHotKeyEvent, HotKeyState};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use toolbox_core::{
    AppProfile, Config, HostCommand, HostStatus, PROTOCOL_VERSION, Platform, Request, Response,
    UiPage, storage::ConfigStore,
};
use toolbox_windows::{WindowCapabilities, WindowService};
use tray_icon::{
    TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    window::WindowId,
};

type Reply = mpsc::Sender<Response>;
pub enum Event {
    Command(Request, Reply),
    Hotkey(GlobalHotKeyEvent),
    Menu(MenuEvent),
    Tray(TrayIconEvent),
    WindowDone(String, Result<String, String>, Option<Reply>),
    Capabilities(WindowCapabilities, Option<Reply>),
    Ui(Result<(), String>),
    Warning(String),
    Quit,
}
enum WindowTask {
    Toggle(AppProfile, Option<Reply>),
    Reset,
    Probe(Option<Reply>),
}

pub fn run(
    config: Config,
    store: ConfigStore,
    server: toolbox_ipc::PipeServer,
    initial: Option<UiPage>,
) -> Result<(), String> {
    let mut builder = EventLoop::<Event>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false)
            .with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("Desktop event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let sender = proxy.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e| {
        let _ = sender.send_event(Event::Hotkey(e));
    }));
    let sender = proxy.clone();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = sender.send_event(Event::Menu(e));
    }));
    let sender = proxy.clone();
    TrayIconEvent::set_event_handler(Some(move |e| {
        let _ = sender.send_event(Event::Tray(e));
    }));

    let mut app = App::new(config, store, proxy.clone(), initial)?;
    let sender = proxy.clone();
    let stopping = Arc::new(AtomicBool::new(false));
    let server_stopping = stopping.clone();
    let listener = thread::spawn(move || {
        while !server_stopping.load(Ordering::Relaxed) {
            let connection = match server.accept() {
                Ok(c) => c,
                Err(e) => {
                    let _ = sender.send_event(Event::Warning(format!("IPC listener stopped: {e}")));
                    break;
                }
            };
            if server_stopping.load(Ordering::Relaxed) {
                break;
            }
            let request = match connection.receive::<Request>() {
                Ok(r) => r,
                Err(e) => {
                    let _ = connection.send(&Response::error(e.to_string()));
                    continue;
                }
            };
            let quitting =
                matches!(request.command, HostCommand::Quit) && request.version == PROTOCOL_VERSION;
            let (tx, rx) = mpsc::channel();
            if sender.send_event(Event::Command(request, tx)).is_err() {
                break;
            }
            let response = rx
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| Response::error("Host operation timed out"));
            let _ = connection.send(&response);
            if quitting && response.ok {
                let _ = sender.send_event(Event::Quit);
                break;
            }
        }
    });
    let result = event_loop.run_app(&mut app).map_err(|e| e.to_string());
    stopping.store(true, Ordering::Relaxed);
    if !listener.is_finished()
        && let Ok(endpoint) = toolbox_ipc::current_user_endpoint(toolbox_core::HOST_ENDPOINT)
    {
        let _ = toolbox_ipc::connect_local(&endpoint, 200);
    }
    let _ = listener.join();
    drop(app);
    result
}

struct App {
    config: Config,
    store: ConfigStore,
    revision: u64,
    status: HostStatus,
    hotkeys: Option<Hotkeys>,
    tray: Option<TrayIcon>,
    window_tx: mpsc::SyncSender<WindowTask>,
    busy: HashSet<String>,
    ui: UiProcess,
    initial: Option<UiPage>,
    executable: PathBuf,
    pressed: HashSet<u32>,
}

impl App {
    fn new(
        config: Config,
        store: ConfigStore,
        proxy: EventLoopProxy<Event>,
        initial: Option<UiPage>,
    ) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let ui_executable = executable.with_file_name(if cfg!(windows) {
            "sean-toolbox-ui.exe"
        } else {
            "sean-toolbox-ui"
        });
        let sender = proxy.clone();
        let ui = UiProcess::new(ui_executable, move |result| {
            let _ = sender.send_event(Event::Ui(result));
        });
        let (window_tx, rx) = mpsc::sync_channel(16);
        thread::spawn(move || {
            let mut windows = WindowService::new();
            let _ = proxy.send_event(Event::Capabilities(windows.capabilities(), None));
            while let Ok(task) = rx.recv() {
                match task {
                    WindowTask::Toggle(profile, reply) => {
                        let result = windows.toggle(&profile);
                        let _ = proxy.send_event(Event::WindowDone(profile.id, result, reply));
                    }
                    WindowTask::Reset => windows.reset(),
                    WindowTask::Probe(reply) => {
                        let _ =
                            proxy.send_event(Event::Capabilities(windows.capabilities(), reply));
                    }
                }
            }
        });
        Ok(Self {
            config,
            store,
            revision: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_micros() as u64,
            status: HostStatus {
                process_id: std::process::id(),
                ..HostStatus::default()
            },
            hotkeys: None,
            tray: None,
            window_tx,
            busy: HashSet::new(),
            ui,
            initial,
            executable,
            pressed: HashSet::new(),
        })
    }

    fn show(&mut self, page: UiPage) -> Result<(), String> {
        self.ui.show(page)
    }

    fn toggle(&mut self, id: &str, reply: Option<Reply>) {
        let result: Result<(), String> = (|| {
            if !self.config.windows_enabled {
                return Err("窗口模块已停用 / Window module is disabled".into());
            }
            let profile = self
                .config
                .profiles
                .iter()
                .find(|p| p.id == id)
                .ok_or("Unknown profile")?;
            if !profile.enabled {
                return Err("此应用配置已停用 / Profile is disabled".into());
            }
            if profile.platform != Platform::current() {
                return Err("此配置属于其他操作系统 / Profile belongs to another platform".into());
            }
            if self.busy.contains(id) {
                return Err(
                    "此应用正在处理上一操作 / Application operation is already in progress".into(),
                );
            }
            self.window_tx
                .try_send(WindowTask::Toggle(profile.clone(), reply.clone()))
                .map_err(|_| "Window worker is busy".to_string())?;
            self.busy.insert(id.into());
            Ok(())
        })();
        if let Err(error) = result {
            self.status.last_action = error.clone();
            if let Some(reply) = reply {
                let _ = reply.send(Response::error(error));
            }
        }
    }

    fn save(&mut self, config: Config, expected_revision: u64) -> Result<(), String> {
        if expected_revision != self.revision {
            return Err(
                "配置已更新，请重新载入再保存 / Configuration changed; reload before saving".into(),
            );
        }
        config.validate()?;
        if !self.busy.is_empty() {
            return Err(
                "请等待当前窗口操作完成后保存 / Wait for the current window operation to finish"
                    .into(),
            );
        }
        let hotkeys = self.hotkeys.as_mut().ok_or("Host is still starting")?;
        let prepared = hotkeys.prepare(&config)?;
        let startup_changed = config.autostart != self.config.autostart;
        if startup_changed && let Err(error) = autostart::set(config.autostart, &self.executable) {
            hotkeys.rollback(prepared);
            return Err(error);
        }
        if let Err(error) = self.store.save(&config) {
            hotkeys.rollback(prepared);
            if startup_changed
                && let Err(rollback) = autostart::set(self.config.autostart, &self.executable)
            {
                self.status.warnings.push(rollback);
            }
            return Err(error);
        }
        let warnings = hotkeys.commit(prepared);
        self.status.hotkeys = hotkeys.available();
        self.status
            .warnings
            .retain(|s| !s.starts_with("Hotkeys:") || !self.status.hotkeys);
        self.status.warnings.extend(warnings);
        let language_changed = self.config.language != config.language;
        self.config = config;
        self.revision += 1;
        self.pressed.clear();
        let _ = self.window_tx.try_send(WindowTask::Reset);
        if language_changed {
            self.create_tray();
        }
        Ok(())
    }

    fn response(&self, message: &str, with_config: bool) -> Response {
        Response {
            ok: true,
            message: message.into(),
            config: with_config.then(|| self.config.clone()),
            status: Some(self.status.clone()),
            revision: self.revision,
        }
    }

    fn command(&mut self, request: Request, reply: Reply) {
        if request.version != PROTOCOL_VERSION {
            let _ = reply.send(Response::error("Unsupported protocol version"));
            return;
        }
        let response = match request.command {
            HostCommand::Ping => self.response("pong", false),
            HostCommand::GetConfig => self.response("Configuration", true),
            HostCommand::Status => {
                if self
                    .window_tx
                    .try_send(WindowTask::Probe(Some(reply.clone())))
                    .is_ok()
                {
                    return;
                }
                self.response(
                    "Window worker is busy; showing last known capabilities",
                    false,
                )
            }
            HostCommand::Show(page) => match self.show(page) {
                Ok(()) => self.response("Opening Sean Toolbox", false),
                Err(e) => Response::error(e),
            },
            HostCommand::SaveConfig {
                config,
                expected_revision,
            } => match self.save(config, expected_revision) {
                Ok(()) => self.response("配置已保存 / Configuration saved", true),
                Err(e) => Response::error(e),
            },
            HostCommand::Toggle { id } => {
                self.toggle(&id, Some(reply));
                return;
            }
            HostCommand::Quit => self.response("Sean Toolbox stopped", false),
        };
        let _ = reply.send(response);
    }

    fn create_tray(&mut self) {
        self.status
            .warnings
            .retain(|warning| !warning.starts_with("Tray:"));
        let zh = self.config.language == "zh-CN";
        let build = || -> Result<TrayIcon, String> {
            let menu = Menu::new();
            for (id, label) in [
                (
                    "projects",
                    if zh {
                        "最近项目"
                    } else {
                        "Recent projects"
                    },
                ),
                (
                    "windows",
                    if zh {
                        "窗口快捷键"
                    } else {
                        "Window shortcuts"
                    },
                ),
                ("settings", if zh { "设置" } else { "Settings" }),
                (
                    "diagnostics",
                    if zh {
                        "诊断与关于"
                    } else {
                        "Diagnostics & about"
                    },
                ),
                (
                    "quit",
                    if zh {
                        "退出工具箱"
                    } else {
                        "Quit Sean Toolbox"
                    },
                ),
            ] {
                menu.append(&MenuItem::with_id(id, label, true, None))
                    .map_err(|e| e.to_string())?;
            }
            let mut rgba = vec![0u8; 32 * 32 * 4];
            for y in 3..29 {
                for x in 3..29 {
                    let handle = (10..22).contains(&x)
                        && (3..9).contains(&y)
                        && !((12..20).contains(&x) && (5..8).contains(&y));
                    let body = (9..28).contains(&y);
                    if handle || body {
                        let color = if (14..18).contains(&x) && (15..21).contains(&y) {
                            [245, 248, 252, 255]
                        } else {
                            [50, 115, 212, 255]
                        };
                        rgba[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4].copy_from_slice(&color);
                    }
                }
            }
            let icon = tray_icon::Icon::from_rgba(rgba, 32, 32).map_err(|e| e.to_string())?;
            TrayIconBuilder::new()
                .with_id("sean-toolbox")
                .with_menu(Box::new(menu))
                .with_tooltip("Sean Toolbox · 个人工具箱")
                .with_icon(icon)
                .with_menu_on_left_click(false)
                .build()
                .map_err(|e| e.to_string())
        };
        match build() {
            Ok(tray) => {
                self.tray = Some(tray);
                self.status.tray = true;
            }
            Err(error) => {
                self.status.tray = false;
                self.status.warnings.push(format!("Tray: {error}"));
            }
        }
    }
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {
        if self.hotkeys.is_some() {
            return;
        }
        let (hotkeys, warnings) = Hotkeys::new(&self.config);
        self.status.hotkeys = hotkeys.available();
        self.status.warnings.extend(warnings);
        self.hotkeys = Some(hotkeys);
        self.create_tray();
        if self.config.autostart
            && let Err(e) = autostart::set(true, &self.executable)
        {
            self.status.warnings.push(e);
        }
        if let Some(page) = self.initial.take()
            && let Err(e) = self.show(page)
        {
            self.status.warnings.push(e);
        }
    }

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        match event {
            Event::Command(request, reply) => self.command(request, reply),
            Event::Hotkey(event) => {
                if event.state == HotKeyState::Released {
                    self.pressed.remove(&event.id);
                    return;
                }
                if !self.pressed.insert(event.id) {
                    return;
                }
                match self.hotkeys.as_ref().and_then(|h| h.binding(event.id)) {
                    Some(Binding::Launcher) => {
                        if let Err(e) = self.show(UiPage::Projects) {
                            self.status.last_action = e;
                        }
                    }
                    Some(Binding::Profile(id)) => self.toggle(&id, None),
                    None => {}
                }
            }
            Event::Menu(event) => {
                let page = match event.id.0.as_str() {
                    "projects" => UiPage::Projects,
                    "windows" => UiPage::Windows,
                    "settings" => UiPage::Settings,
                    "diagnostics" => UiPage::Diagnostics,
                    "quit" => {
                        event_loop.exit();
                        return;
                    }
                    _ => return,
                };
                if let Err(e) = self.show(page) {
                    self.status.last_action = e;
                }
            }
            Event::Tray(TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            }) => {
                if let Err(e) = self.show(UiPage::Projects) {
                    self.status.last_action = e;
                }
            }
            Event::Tray(_) => {}
            Event::WindowDone(id, result, reply) => {
                self.busy.remove(&id);
                let response = match result {
                    Ok(message) => {
                        self.status.last_action = message.clone();
                        self.response(&message, false)
                    }
                    Err(error) => {
                        self.status.last_action = error.clone();
                        Response::error(error)
                    }
                };
                if let Some(reply) = reply {
                    let _ = reply.send(response);
                }
            }
            Event::Capabilities(capabilities, reply) => {
                self.status.backend = capabilities.backend;
                self.status.window_control = capabilities.available;
                self.status.warnings.retain(|w| !w.starts_with("Windows:"));
                if let Some(reason) = capabilities.reason {
                    self.status.warnings.push(format!("Windows: {reason}"));
                }
                if let Some(reply) = reply {
                    let _ = reply.send(self.response("Host status", false));
                }
            }
            Event::Ui(result) => {
                self.status.warnings.retain(|w| !w.starts_with("UI:"));
                if let Err(error) = result {
                    self.status.warnings.push(format!("UI: {error}"));
                }
            }
            Event::Warning(warning) => self.status.warnings.push(warning),
            Event::Quit => event_loop.exit(),
        }
    }
}
