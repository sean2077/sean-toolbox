#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod model;
mod worker;

use model::{
    ImportedConfig, launch_command, next_profile_id, page_from_index, page_index,
    platform_from_index, platform_index, platform_name, quake_alignment_from_index,
    quake_alignment_index, quake_edge_from_index, quake_edge_index, quake_monitor_from_index,
    quake_monitor_index, theme_from_index, theme_index,
};
use slint::{
    ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel,
    winit_030::{EventResult, WinitWindowAccessor, winit},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};
use toolbox_core::{
    AppProfile, Config, HostStatus, LaunchCommand, Platform, QuakeConfig, Response,
    ThemePreference, UiPage,
};
use worker::{DialogKind, Event, SaveKind, Work};

slint::include_modules!();

thread_local! { static APP: RefCell<Option<Rc<Controller>>> = const { RefCell::new(None) }; }

fn with_controller(action: impl FnOnce(&Controller)) {
    let controller = APP.with(|app| app.borrow().clone());
    if let Some(controller) = controller {
        action(&controller);
        controller.sync_launcher();
    }
}

pub(crate) fn post(event: Event) {
    let _ = slint::invoke_from_event_loop(move || {
        with_controller(|controller| controller.handle(event))
    });
}

#[derive(Clone)]
enum ConfirmAction {
    Close,
    Edit(i32),
    New,
    CancelEdit,
    ResetSettings,
    Delete,
    Reload,
    Import,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectOpenTarget {
    Manager,
    Launcher(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DisplayArea {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    monitor_origin: (i32, i32),
}

fn centered_position(area: DisplayArea, width: u32, height: u32) -> (i32, i32) {
    (
        area.x + (area.width.saturating_sub(width) / 2) as i32,
        area.y + (area.height.saturating_sub(height) / 2) as i32,
    )
}

#[cfg(windows)]
fn active_display_area() -> Option<DisplayArea> {
    use windows_sys::Win32::{
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::WindowsAndMessaging::GetForegroundWindow,
    };
    // Capture the user's foreground display before showing/focusing our popup.
    let monitor = unsafe { MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_null() || unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    Some(DisplayArea {
        x: info.rcWork.left,
        y: info.rcWork.top,
        width: (info.rcWork.right - info.rcWork.left).max(0) as u32,
        height: (info.rcWork.bottom - info.rcWork.top).max(0) as u32,
        monitor_origin: (info.rcMonitor.left, info.rcMonitor.top),
    })
}

#[cfg(not(windows))]
fn active_display_area() -> Option<DisplayArea> {
    None
}

#[cfg(target_os = "linux")]
fn x11_window_has_focus(window: &winit::window::Window) -> Option<bool> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

    let id = match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok()?,
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        _ => return None,
    };
    // Keyboard grabs synthesize FocusOut without moving the actual input
    // focus. Query the server first: the WM's _NET_ACTIVE_WINDOW can still
    // name us after a real FocusOut, with no later event to correct that read.
    let (connection, _) = x11rb::connect(None).ok()?;
    let mut focus = connection.get_input_focus().ok()?.reply().ok()?.focus;
    if focus == id {
        return Some(true);
    }
    if focus == x11rb::NONE {
        return Some(false);
    }
    let root = connection.query_tree(id).ok()?.reply().ok()?.root;
    // PointerRoot (1) has no concrete focus window; only this case needs the
    // WM's active-window hint. Otherwise include native child windows in the
    // same focus family, with a bounded walk for malformed window trees.
    if focus != 1 {
        for _ in 0..64 {
            if focus == id {
                return Some(true);
            }
            if focus == root || focus == x11rb::NONE {
                return Some(false);
            }
            focus = connection.query_tree(focus).ok()?.reply().ok()?.parent;
        }
        return None;
    }
    let atom = connection
        .intern_atom(true, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    if atom != 0 {
        let property = connection
            .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        if let Some(active) = property.value32().and_then(|mut values| values.next()) {
            return Some(active == id);
        }
    }
    None
}

struct State {
    config: Config,
    revision: u64,
    editor: Option<AppProfile>,
    editor_index: Option<usize>,
    candidate: usize,
    imported: Option<ImportedConfig>,
    confirmation: Option<ConfirmAction>,
    dialog_open: bool,
    visible: bool,
    release_requested: bool,
    exit_requested: bool,
    launcher_visible: bool,
    launcher_had_focus: bool,
    launcher_session: u64,
    launcher_placement_pending: bool,
    launcher_placement_attempts: u8,
    launcher_display: Option<DisplayArea>,
    project_open_target: Option<ProjectOpenTarget>,
    toggling: bool,
    rendered_generation: u64,
    host_pid: u32,
    host_status: Option<HostStatus>,
    last_error: String,
}

struct Controller {
    ui: slint::Weak<AppWindow>,
    launcher: slint::Weak<LauncherWindow>,
    state: RefCell<State>,
    work: mpsc::Sender<Work>,
    query_generation: Arc<AtomicU64>,
    idle_timer: Timer,
}

impl Controller {
    fn ui(&self) -> AppWindow {
        self.ui
            .upgrade()
            .expect("UI exists while controller is installed")
    }

    fn launcher(&self) -> LauncherWindow {
        self.launcher
            .upgrade()
            .expect("Launcher exists while controller is installed")
    }

    fn sync_launcher(&self) {
        let ui = self.ui();
        let launcher = self.launcher();
        launcher.set_english(ui.get_english());
        launcher.set_loaded(ui.get_loaded());
        launcher.set_projects_enabled(ui.get_projects_enabled());
        launcher.set_projects(ui.get_projects());
        launcher.set_query(ui.get_query());
        launcher.set_selected_project(ui.get_selected_project());
        launcher.set_project_message(ui.get_project_message());
        launcher.set_project_loading(ui.get_project_loading());
        launcher.set_project_opening(ui.get_project_opening());
        launcher.set_project_error(ui.get_project_error());
        launcher.set_project_total(ui.get_project_total());
        launcher.set_status_message(ui.get_status_message());
        launcher.set_status_error(ui.get_status_error());
        launcher.invoke_reveal_selection();
    }

    fn show_launcher(&self, refresh: bool) -> Result<(), String> {
        self.idle_timer.stop();
        let ui = self.ui();
        {
            let mut state = self.state.borrow_mut();
            state.launcher_had_focus &= state.launcher_visible;
            state.launcher_visible = true;
            state.release_requested = false;
            state.launcher_session = state.launcher_session.wrapping_add(1);
            state.launcher_placement_pending = true;
            state.launcher_placement_attempts = 0;
            state.launcher_display = active_display_area();
        }
        ui.set_query("".into());
        ui.set_selected_project(-1);
        ui.set_status_error(false);
        self.query_generation.fetch_add(1, Ordering::Relaxed);
        self.sync_launcher();
        let launcher = self.launcher();
        launcher.window().set_minimized(false);
        if let Err(error) = launcher.show() {
            self.state.borrow_mut().launcher_visible = false;
            return Err(error.to_string());
        }
        self.activate_launcher();
        // The native window may not exist until the first event-loop turn.
        // A stale scheduled activation must never resurrect a dismissed popup.
        let session = self.state.borrow().launcher_session;
        Timer::single_shot(Duration::ZERO, move || {
            with_controller(|controller| {
                if controller.state.borrow().launcher_session == session {
                    controller.activate_launcher();
                }
            });
        });
        if refresh {
            self.send(Work::Load { force: false });
        } else if ui.get_loaded() {
            self.refresh_projects();
        }
        self.sync_launcher();
        launcher.invoke_focus_search();
        Ok(())
    }

    fn activate_launcher(&self) {
        if !self.state.borrow().launcher_visible {
            return;
        }
        self.place_launcher();
        self.apply_native_theme();
        let launcher = self.launcher();
        let focused = launcher
            .window()
            .with_winit_window(|window| {
                #[cfg(windows)]
                {
                    use winit::platform::windows::WindowExtWindows;
                    window.set_skip_taskbar(true);
                }
                window.focus_window();
                window.has_focus()
            })
            .unwrap_or(false);
        if focused {
            self.state.borrow_mut().launcher_had_focus = true;
        }
        launcher.invoke_focus_search();
    }

    fn place_launcher(&self) {
        let (visible, pending, requested_area, attempts) = {
            let state = self.state.borrow();
            (
                state.launcher_visible,
                state.launcher_placement_pending,
                state.launcher_display,
                state.launcher_placement_attempts,
            )
        };
        if !visible || !pending {
            return;
        }
        let launcher = self.launcher();
        let mut resize_request = None;
        let placed = launcher.window().with_winit_window(|window| {
            let monitor = requested_area
                .and_then(|area| {
                    window.available_monitors().find(|monitor| {
                        let position = monitor.position();
                        (position.x, position.y) == area.monitor_origin
                    })
                })
                .or_else(|| window.current_monitor())
                .or_else(|| window.primary_monitor());
            let Some(monitor) = monitor else {
                return false;
            };
            let position = monitor.position();
            let size = monitor.size();
            let area = requested_area.unwrap_or(DisplayArea {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                monitor_origin: (position.x, position.y),
            });
            let scale = if std::env::var_os("SLINT_SCALE_FACTOR").is_some() {
                f64::from(launcher.window().scale_factor())
            } else {
                monitor.scale_factor()
            };
            let width =
                ((f64::from(launcher.get_panel_width()) * scale).round() as u32).min(area.width);
            let height =
                ((f64::from(launcher.get_panel_height()) * scale).round() as u32).min(area.height);
            let size = winit::dpi::PhysicalSize::new(width, height);
            if window.inner_size() != size {
                // X11 locks a non-resizable window to its current physical
                // size. Release that constraint before requesting a new size.
                window.set_resizable(true);
            }
            let (x, y) = centered_position(area, width, height);
            window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
            // Move before resizing: Windows rescales the old physical size in
            // WM_DPICHANGED. Resizing on the old display would apply DPI twice.
            if (window.scale_factor() - monitor.scale_factor()).abs() > f64::EPSILON {
                return false;
            }
            if window.inner_size() != size {
                if attempts < 4 {
                    // Keep placement pending until Resized confirms the size;
                    // locking here would freeze the old minimum on X11.
                    resize_request = Some(slint::PhysicalSize::new(width, height));
                    return false;
                }
                // A small display or the window manager may enforce a larger
                // minimum. Center the actual window without a resize loop.
                let actual = window.inner_size();
                let (x, y) = centered_position(area, actual.width, actual.height);
                window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
            }
            window.set_resizable(false);
            true
        });
        if let Some(size) = resize_request {
            self.state.borrow_mut().launcher_placement_attempts += 1;
            // Slint records an explicit, in-flight size request, preventing a
            // later property update from restoring obsolete logical dimensions.
            launcher.window().set_size(size);
        }
        if placed == Some(true) {
            self.state.borrow_mut().launcher_placement_pending = false;
        }
    }

    fn queue_launcher_placement(&self) {
        let session = {
            let mut state = self.state.borrow_mut();
            if !state.launcher_visible {
                return;
            }
            if !state.launcher_placement_pending {
                state.launcher_placement_attempts = 0;
            }
            state.launcher_placement_pending = true;
            state.launcher_session
        };
        // Winit filters run before Slint processes DPI/size events. Apply the
        // final logical size and centering after both backends finish the event.
        Timer::single_shot(Duration::ZERO, move || {
            with_controller(|controller| {
                if controller.state.borrow().launcher_session == session {
                    controller.place_launcher();
                }
            });
        });
    }

    fn launcher_focus_changed(&self, focused: bool) {
        let dismiss = {
            let mut state = self.state.borrow_mut();
            if !state.launcher_visible {
                return;
            }
            if focused {
                state.launcher_had_focus = true;
                false
            } else {
                state.launcher_had_focus
            }
        };
        if dismiss {
            #[cfg(target_os = "linux")]
            if self
                .launcher()
                .window()
                .with_winit_window(x11_window_has_focus)
                .flatten()
                == Some(true)
            {
                return;
            }
            self.hide_launcher();
        }
    }

    fn queue_launcher_focus_loss(&self) {
        let session = self.state.borrow().launcher_session;
        Timer::single_shot(Duration::ZERO, move || {
            with_controller(|controller| {
                if controller.state.borrow().launcher_session != session {
                    return;
                }
                let focused = controller
                    .launcher()
                    .window()
                    .with_winit_window(|window| {
                        #[cfg(target_os = "linux")]
                        if let Some(focused) = x11_window_has_focus(window) {
                            return focused;
                        }
                        window.has_focus()
                    })
                    .unwrap_or(false);
                controller.launcher_focus_changed(focused);
            });
        });
    }

    fn hide_launcher(&self) {
        {
            let mut state = self.state.borrow_mut();
            state.launcher_visible = false;
            state.launcher_had_focus = false;
            state.launcher_placement_pending = false;
        }
        if let Err(error) = self.launcher().hide() {
            self.error(error.to_string());
        }
        self.schedule_unload();
    }

    fn open_launcher_settings(&self) {
        if let Err(error) = self.show(UiPage::Settings, false) {
            self.error(error);
        }
    }

    fn close_launcher(&self) {
        // Explicit dismissal releases the UI even when automatic dismissal has
        // a configured warm-cache delay. Other surfaces/drafts still guard it.
        self.state.borrow_mut().release_requested = true;
        self.hide_launcher();
    }

    fn send(&self, work: Work) {
        if self.work.send(work).is_err() {
            let ui = self.ui();
            ui.set_loaded(false);
            ui.set_saving(false);
            ui.set_project_loading(false);
            ui.set_project_opening(false);
            ui.set_dialog_busy(false);
            ui.set_diagnostic_loading(false);
            ui.set_diagnostic_loaded(false);
            {
                let mut state = self.state.borrow_mut();
                state.dialog_open = false;
                state.toggling = false;
            }
            self.error(
                "后台任务已停止，请重新打开工具箱 / Background worker stopped; reopen the toolbox",
            );
        }
    }

    fn say(&self, chinese: &str, english: &str) {
        let ui = self.ui();
        ui.set_status_message(if ui.get_english() { english } else { chinese }.into());
        ui.set_status_error(false);
    }

    fn error(&self, message: impl Into<String>) {
        let message = message.into();
        self.state.borrow_mut().last_error.clone_from(&message);
        let ui = self.ui();
        ui.set_status_message(message.into());
        ui.set_status_error(true);
        self.sync_diagnostics();
    }

    fn dirty(&self) -> bool {
        let ui = self.ui();
        ui.get_settings_dirty() || ui.get_profile_dirty()
    }

    fn show(&self, page: UiPage, refresh: bool) -> Result<(), String> {
        if self.state.borrow().exit_requested {
            // quit_event_loop queues a terminal event; showing a window cannot
            // retract it. Let the supervisor replace us after confirmed exit.
            return Err("UI is closing".into());
        }
        let ui = self.ui();
        // Activation must not move the page underneath an open modal dialog.
        let modal = ui.get_confirm_visible() || ui.get_import_visible() || ui.get_dialog_busy();
        if page == UiPage::Projects && !modal {
            return self.show_launcher(refresh);
        }
        self.idle_timer.stop();
        {
            let mut state = self.state.borrow_mut();
            state.visible = true;
            state.release_requested = false;
        }
        self.hide_launcher();
        if !modal {
            ui.set_page(page_index(page));
        }
        ui.window().set_minimized(false);
        ui.show().map_err(|e| e.to_string())?;
        self.apply_native_theme();
        ui.window()
            .with_winit_window(|window| window.focus_window());
        ui.invoke_focus_page();
        if modal {
            return Ok(());
        }
        if ui.get_page() == page_index(UiPage::Projects) {
            ui.set_query("".into());
            ui.set_selected_project(-1);
            self.query_generation.fetch_add(1, Ordering::Relaxed);
            if ui.get_loaded() && !refresh {
                self.refresh_projects();
            }
        }
        if refresh {
            self.send(Work::Load { force: false });
        }
        if page == UiPage::Diagnostics {
            self.refresh_status();
        }
        Ok(())
    }

    fn navigate(&self, page: i32) {
        let ui = self.ui();
        if ui.get_confirm_visible() || ui.get_import_visible() || ui.get_dialog_busy() {
            return;
        }
        let target = page_index(page_from_index(page));
        let changed = target != ui.get_page();
        ui.set_page(target);
        ui.invoke_focus_page();
        if changed && target == 0 && ui.get_loaded() {
            self.refresh_projects();
        } else if changed && target == 3 {
            self.refresh_status();
        }
    }

    fn dismiss(&self) {
        let ui = self.ui();
        if ui.get_saving() || ui.get_project_opening() || self.state.borrow().dialog_open {
            self.say(
                "请等待当前操作完成。",
                "Wait for the current operation to finish.",
            );
        } else if ui.get_confirm_visible() {
            self.confirm(false);
        } else if ui.get_import_visible() {
            self.cancel_import();
        } else if self.dirty() {
            self.ask(
                ConfirmAction::Close,
                "放弃尚未保存的修改？",
                "Discard unsaved changes?",
                "关闭界面会释放内存，尚未保存的修改会丢失。",
                "Closing releases the UI from memory; unsaved changes will be lost.",
            );
        } else {
            self.close_manager();
        }
    }

    fn close_manager(&self) {
        self.state.borrow_mut().release_requested = true;
        self.hide();
    }

    fn hide(&self) {
        if let Err(error) = self.ui().hide() {
            self.error(error.to_string());
            return;
        }
        self.state.borrow_mut().visible = false;
        self.schedule_unload();
    }

    fn unload_delay(&self) -> Option<Duration> {
        let state = self.state.borrow();
        if state.exit_requested
            || state.visible
            || state.launcher_visible
            || state.toggling
            || state.dialog_open
            || self.ui().get_project_opening()
            || self.ui().get_saving()
            || self.dirty()
        {
            return None;
        }
        Some(Duration::from_secs(if state.release_requested {
            0
        } else {
            state.config.ui_idle_seconds
        }))
    }

    fn schedule_unload(&self) {
        self.idle_timer.stop();
        if let Some(delay) = self.unload_delay() {
            // Recheck at the event-loop boundary: a new summon or operation may
            // arrive after dismissal but before this timer is dispatched.
            self.idle_timer.start(TimerMode::SingleShot, delay, || {
                with_controller(|controller| {
                    if controller.unload_delay().is_some() {
                        controller.state.borrow_mut().exit_requested = true;
                        if slint::quit_event_loop().is_err() {
                            controller.state.borrow_mut().exit_requested = false;
                        }
                    }
                });
            });
        }
    }

    fn sync_settings(&self) {
        let ui = self.ui();
        let state = self.state.borrow();
        let config = &state.config;
        ui.set_english(config.language == "en");
        ui.set_setting_language(i32::from(config.language == "en"));
        ui.set_setting_theme(theme_index(config.theme));
        self.apply_theme();
        ui.set_setting_hotkey(config.launcher_hotkey.clone().into());
        ui.set_setting_autostart(config.autostart);
        ui.set_setting_projects(config.projects_enabled);
        ui.set_setting_windows(config.windows_enabled);
        ui.set_setting_idle(config.ui_idle_seconds.to_string().into());
        ui.set_setting_code_path(
            config
                .vscode_path_override
                .clone()
                .unwrap_or_default()
                .into(),
        );
        ui.set_settings_dirty(false);
        self.update_match_help();
        self.sync_candidate_labels();
        self.sync_diagnostics();
    }

    fn sync_profiles(&self) {
        let ui = self.ui();
        ui.set_projects_enabled(self.state.borrow().config.projects_enabled);
        ui.set_windows_enabled(self.state.borrow().config.windows_enabled);
        let rows = self
            .state
            .borrow()
            .config
            .profiles
            .iter()
            .map(|profile| ProfileRow {
                name: profile.name.clone().into(),
                hotkey: profile.hotkey.clone().into(),
                platform: platform_name(profile.platform).into(),
                detail: format!(
                    "{}  ·  {}",
                    platform_name(profile.platform),
                    if profile.hotkey.is_empty() {
                        "—"
                    } else {
                        &profile.hotkey
                    }
                )
                .into(),
                enabled: profile.enabled,
                available: profile.platform == Platform::current(),
            })
            .collect::<Vec<_>>();
        ui.set_profiles(ModelRc::new(VecModel::from(rows)));
    }

    fn refresh_projects(&self) {
        let ui = self.ui();
        if !ui.get_loaded() {
            return;
        }
        let state = self.state.borrow();
        let generation = self.query_generation.fetch_add(1, Ordering::Relaxed) + 1;
        ui.set_project_error(false);
        if !state.config.projects_enabled {
            ui.set_projects(ModelRc::default());
            ui.set_selected_project(-1);
            ui.set_project_loading(false);
            ui.set_project_total(0);
            ui.set_project_message(
                if ui.get_english() {
                    "Projects are disabled. Enable this module in Settings."
                } else {
                    "最近项目模块已关闭，可在设置中启用。"
                }
                .into(),
            );
            return;
        }
        ui.set_project_loading(true);
        ui.set_selected_project(-1);
        ui.set_project_message(
            if ui.get_english() {
                "Reading recent projects…"
            } else {
                "正在读取最近项目…"
            }
            .into(),
        );
        let code_path = state.config.vscode_path_override.clone();
        drop(state);
        self.send(Work::Discover {
            generation,
            query: ui.get_query().into(),
            code_path,
        });
    }

    fn query_changed(&self, query: String) {
        let ui = self.ui();
        // A cold launcher accepts typing while its saved configuration loads.
        // Discovery will consume this exact query once the worker is ready.
        ui.set_query(query.clone().into());
        if !self.ui().get_loaded() || !self.state.borrow().config.projects_enabled {
            return;
        }
        let generation = self.query_generation.fetch_add(1, Ordering::Relaxed) + 1;
        ui.set_selected_project(-1);
        ui.set_project_loading(true);
        ui.set_project_error(false);
        self.send(Work::Search { generation, query });
    }

    fn move_project(&self, delta: i32) {
        let ui = self.ui();
        let count = ui.get_projects().row_count() as i32;
        if count == 0 || ui.get_project_loading() {
            return;
        }
        ui.set_selected_project((ui.get_selected_project() + delta).clamp(0, count - 1));
        ui.invoke_reveal_selection();
    }

    fn open_project(&self, index: i32) {
        self.request_project_open(index, ProjectOpenTarget::Manager);
    }

    fn select_launcher_project(&self, index: i32) {
        let ui = self.ui();
        if !ui.get_project_loading()
            && index >= 0
            && (index as usize) < ui.get_projects().row_count()
        {
            ui.set_selected_project(index);
            self.sync_launcher();
        }
    }

    fn open_launcher_project(&self, index: i32) {
        let state = self.state.borrow();
        if !state.launcher_visible {
            return;
        }
        let session = state.launcher_session;
        drop(state);
        self.request_project_open(index, ProjectOpenTarget::Launcher(session));
    }

    fn request_project_open(&self, index: i32, target: ProjectOpenTarget) {
        let ui = self.ui();
        if target == ProjectOpenTarget::Manager && self.dirty() {
            self.say(
                "请先保存或取消修改，再打开项目。",
                "Save or cancel your changes before opening a project.",
            );
            return;
        }
        if ui.get_project_opening()
            || ui.get_project_loading()
            || !ui.get_loaded()
            || ui.get_saving()
            || ui.get_confirm_visible()
            || ui.get_import_visible()
            || ui.get_dialog_busy()
            || index < 0
            || !self.state.borrow().config.projects_enabled
            || self.state.borrow().rendered_generation
                != self.query_generation.load(Ordering::Relaxed)
        {
            return;
        }
        let Some(item) = ui.get_projects().row_data(index as usize) else {
            return;
        };
        ui.set_selected_project(index);
        ui.set_project_opening(true);
        self.state.borrow_mut().project_open_target = Some(target);
        let code_path = self.state.borrow().config.vscode_path_override.clone();
        self.send(Work::Open {
            id: item.id.into(),
            code_path,
        });
    }

    fn settings_changed(&self) {
        let ui = self.ui();
        ui.set_settings_dirty(true);
        ui.set_english(ui.get_setting_language() == 1);
        self.apply_theme();
        self.update_match_help();
        self.sync_candidate_labels();
        self.sync_diagnostics();
    }

    fn apply_theme(&self) {
        let scheme = match theme_from_index(self.ui().get_setting_theme()) {
            ThemePreference::System => slint::language::ColorScheme::Unknown,
            ThemePreference::Light => slint::language::ColorScheme::Light,
            ThemePreference::Dark => slint::language::ColorScheme::Dark,
        };
        self.ui().global::<Palette>().set_color_scheme(scheme);
        self.launcher().global::<Palette>().set_color_scheme(scheme);
        self.apply_native_theme();
    }

    fn apply_native_theme(&self) {
        let native = match theme_from_index(self.ui().get_setting_theme()) {
            ThemePreference::System => None,
            ThemePreference::Light => Some(winit::window::Theme::Light),
            ThemePreference::Dark => Some(winit::window::Theme::Dark),
        };
        for window in [self.ui().window(), self.launcher().window()] {
            window.with_winit_window(|window| window.set_theme(native));
        }
    }

    fn reset_settings(&self) {
        if self.ui().get_saving() || self.state.borrow().dialog_open {
            return;
        }
        if self.ui().get_settings_dirty() {
            self.ask(
                ConfirmAction::ResetSettings,
                "放弃设置的修改？",
                "Discard settings changes?",
                "将恢复已保存的设置，应用配置的修改会保留。",
                "Restore saved settings. Application profile edits will be kept.",
            );
        }
    }

    fn settings_config(&self) -> Result<Config, String> {
        let ui = self.ui();
        let mut config = self.state.borrow().config.clone();
        config.language = if ui.get_setting_language() == 1 {
            "en"
        } else {
            "zh-CN"
        }
        .into();
        config.theme = theme_from_index(ui.get_setting_theme());
        config.launcher_hotkey = ui.get_setting_hotkey().trim().to_owned();
        config.autostart = ui.get_setting_autostart();
        config.projects_enabled = ui.get_setting_projects();
        config.windows_enabled = ui.get_setting_windows();
        config.ui_idle_seconds = ui.get_setting_idle().trim().parse().map_err(|_| {
            "释放时间必须为 0–3600 的整数 / Idle timeout must be an integer from 0 to 3600"
                .to_owned()
        })?;
        let code = ui.get_setting_code_path().trim().to_owned();
        config.vscode_path_override = (!code.is_empty()).then_some(code);
        config.validate()?;
        Ok(config)
    }

    fn save(&self, config: Config, kind: SaveKind) {
        if self.ui().get_saving() || !self.ui().get_loaded() {
            return;
        }
        if let Err(error) = config.validate() {
            self.error(error);
            return;
        }
        self.ui().set_saving(true);
        let revision = self.state.borrow().revision;
        self.say("正在保存并应用…", "Saving and applying…");
        self.send(Work::Save {
            config,
            revision,
            kind,
        });
    }

    fn save_settings(&self) {
        match self.settings_config() {
            Ok(config) => self.save(config, SaveKind::Settings),
            Err(error) => self.error(error),
        }
    }

    fn update_match_help(&self) {
        let ui = self.ui();
        let platform = platform_from_index(ui.get_form_platform());
        let (zh, en) = match platform {
            Platform::Windows => (
                "填写进程文件名，例如 Code.exe。",
                "Enter the executable filename, e.g. Code.exe.",
            ),
            Platform::Macos => (
                "匹配 bundle ID、应用名称或可执行文件名；需要辅助功能权限。",
                "Match a bundle ID, app name or executable name. Accessibility permission is required.",
            ),
            Platform::Linux => (
                "X11：匹配 WM_CLASS 或进程文件名。Wayland 窗口控制受限。",
                "X11: match WM_CLASS or executable name. Wayland window control is limited.",
            ),
        };
        ui.set_profile_help(if ui.get_english() { en } else { zh }.into());
    }

    fn profile_changed(&self) {
        let ui = self.ui();
        ui.set_profile_dirty(true);
        if ui.get_form_platform() != 0 {
            ui.set_form_quake_auto_hide(false);
        }
        if ui.get_form_platform() == 1 {
            ui.set_form_quake_topmost(false);
        }
        self.update_match_help();
    }

    fn edit_profile(&self, index: i32) {
        if self.ui().get_saving() || self.state.borrow().dialog_open {
            return;
        }
        if self.ui().get_editing() && self.ui().get_selected_profile() == index {
            return;
        }
        if self.ui().get_profile_dirty() {
            self.ask(
                ConfirmAction::Edit(index),
                "放弃应用的修改？",
                "Discard app changes?",
                "当前应用尚未保存。",
                "The current app has unsaved changes.",
            );
        } else {
            self.begin_edit(index);
        }
    }

    fn begin_edit(&self, index: i32) {
        let profile = if index < 0 {
            let state = self.state.borrow();
            AppProfile {
                id: next_profile_id(&state.config.profiles),
                name: String::new(),
                platform: Platform::current(),
                matcher: String::new(),
                hotkey: String::new(),
                enabled: true,
                maximize: false,
                cycle_continuous: true,
                quake: QuakeConfig::default(),
                launch: vec![LaunchCommand {
                    executable: String::new(),
                    args: vec![],
                    cwd: None,
                }],
            }
        } else {
            let state = self.state.borrow();
            let Some(profile) = state.config.profiles.get(index as usize) else {
                return;
            };
            profile.clone()
        };
        let ui = self.ui();
        ui.set_editing(true);
        ui.set_selected_profile(index);
        ui.set_form_id(profile.id.clone().into());
        ui.set_form_name(profile.name.clone().into());
        ui.set_form_platform(platform_index(profile.platform));
        ui.set_form_matcher(profile.matcher.clone().into());
        ui.set_form_hotkey(profile.hotkey.clone().into());
        ui.set_form_enabled(profile.enabled);
        ui.set_form_maximize(profile.maximize);
        ui.set_form_cycle(profile.cycle_continuous);
        ui.set_form_quake_enabled(profile.quake.enabled);
        ui.set_form_quake_edge(quake_edge_index(profile.quake.edge));
        ui.set_form_quake_alignment(quake_alignment_index(profile.quake.alignment));
        ui.set_form_quake_width(i32::from(profile.quake.width_percent));
        ui.set_form_quake_height(i32::from(profile.quake.height_percent));
        ui.set_form_quake_monitor(quake_monitor_index(profile.quake.monitor));
        ui.set_form_quake_topmost(profile.quake.always_on_top);
        ui.set_form_quake_auto_hide(profile.quake.hide_on_focus_loss);
        ui.set_profile_dirty(false);
        {
            let mut state = self.state.borrow_mut();
            state.editor = Some(profile);
            state.editor_index = (index >= 0).then_some(index as usize);
            state.candidate = 0;
        }
        self.sync_candidate();
        self.update_match_help();
        ui.invoke_focus_page();
    }

    fn new_profile(&self) {
        if self.ui().get_saving() || self.state.borrow().dialog_open {
            return;
        }
        if self.ui().get_profile_dirty() {
            self.ask(
                ConfirmAction::New,
                "放弃应用的修改？",
                "Discard app changes?",
                "当前应用尚未保存。",
                "The current app has unsaved changes.",
            );
        } else {
            self.begin_edit(-1);
        }
    }

    fn capture_candidate(&self) -> Result<(), String> {
        let ui = self.ui();
        let command = launch_command(
            &ui.get_launch_executable(),
            &ui.get_launch_args(),
            &ui.get_launch_cwd(),
        )?;
        let mut state = self.state.borrow_mut();
        let index = state.candidate;
        if let Some(editor) = &mut state.editor {
            editor.launch[index] = command;
        }
        Ok(())
    }

    fn sync_candidate(&self) {
        self.sync_candidate_labels();
        let ui = self.ui();
        let state = self.state.borrow();
        let Some(editor) = &state.editor else {
            return;
        };
        ui.set_candidate_index(state.candidate as i32);
        let command = &editor.launch[state.candidate];
        ui.set_launch_executable(command.executable.clone().into());
        ui.set_launch_args(
            serde_json::to_string(&command.args)
                .unwrap_or_else(|_| "[]".into())
                .into(),
        );
        ui.set_launch_cwd(command.cwd.clone().unwrap_or_default().into());
    }

    fn sync_candidate_labels(&self) {
        let ui = self.ui();
        let state = self.state.borrow();
        let Some(editor) = &state.editor else {
            return;
        };
        let labels = editor
            .launch
            .iter()
            .enumerate()
            .map(|(index, _)| {
                SharedString::from(format!(
                    "{} {}",
                    if ui.get_english() {
                        "Candidate"
                    } else {
                        "候选"
                    },
                    index + 1
                ))
            })
            .collect::<Vec<_>>();
        ui.set_candidate_labels(ModelRc::new(VecModel::from(labels)));
    }

    fn select_candidate(&self, index: i32) {
        if self.state.borrow().dialog_open {
            self.ui()
                .set_candidate_index(self.state.borrow().candidate as i32);
            return;
        }
        if let Err(error) = self.capture_candidate() {
            self.ui()
                .set_candidate_index(self.state.borrow().candidate as i32);
            self.error(error);
            return;
        }
        let count = self
            .state
            .borrow()
            .editor
            .as_ref()
            .map_or(0, |editor| editor.launch.len());
        if index < 0 || index as usize >= count {
            return;
        }
        self.state.borrow_mut().candidate = index as usize;
        self.sync_candidate();
    }

    fn add_candidate(&self) {
        if let Err(error) = self.capture_candidate() {
            self.error(error);
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            let Some(editor) = &mut state.editor else {
                return;
            };
            if editor.launch.len() >= 8 {
                return;
            }
            editor.launch.push(LaunchCommand {
                executable: String::new(),
                args: vec![],
                cwd: None,
            });
            state.candidate = editor.launch.len() - 1;
        }
        self.ui().set_profile_dirty(true);
        self.sync_candidate();
    }

    fn remove_candidate(&self) {
        {
            let mut state = self.state.borrow_mut();
            let index = state.candidate;
            let Some(editor) = &mut state.editor else {
                return;
            };
            if editor.launch.len() <= 1 {
                return;
            }
            editor.launch.remove(index);
            state.candidate = index.min(editor.launch.len() - 1);
        }
        self.ui().set_profile_dirty(true);
        self.sync_candidate();
    }

    fn save_profile(&self) {
        if let Err(error) = self.capture_candidate() {
            self.error(error);
            return;
        }
        let ui = self.ui();
        let (mut config, mut profile, index) = {
            let state = self.state.borrow();
            let Some(editor) = state.editor.clone() else {
                return;
            };
            (state.config.clone(), editor, state.editor_index)
        };
        profile.id = ui.get_form_id().trim().to_owned();
        profile.name = ui.get_form_name().trim().to_owned();
        profile.platform = platform_from_index(ui.get_form_platform());
        profile.matcher = ui.get_form_matcher().trim().to_owned();
        profile.hotkey = ui.get_form_hotkey().trim().to_owned();
        profile.enabled = ui.get_form_enabled();
        profile.maximize = ui.get_form_maximize();
        profile.cycle_continuous = ui.get_form_cycle();
        let width = ui.get_form_quake_width();
        let height = ui.get_form_quake_height();
        if !(10..=100).contains(&width) || !(10..=100).contains(&height) {
            self.error("停靠宽高必须为 10–100% / Quake dimensions must be between 10 and 100%");
            return;
        }
        profile.quake = QuakeConfig {
            enabled: ui.get_form_quake_enabled(),
            edge: quake_edge_from_index(ui.get_form_quake_edge()),
            alignment: quake_alignment_from_index(ui.get_form_quake_alignment()),
            width_percent: width as u8,
            height_percent: height as u8,
            monitor: quake_monitor_from_index(ui.get_form_quake_monitor()),
            always_on_top: ui.get_form_quake_topmost(),
            hide_on_focus_loss: ui.get_form_quake_auto_hide(),
        };
        if let Some(index) = index {
            let Some(existing) = config.profiles.get_mut(index) else {
                self.error("应用列表已变化，请重新载入 / App list changed; reload configuration");
                return;
            };
            *existing = profile;
        } else {
            config.profiles.push(profile);
        }
        self.save(config, SaveKind::Profile);
    }

    fn cancel_editor(&self) {
        self.state.borrow_mut().editor = None;
        self.state.borrow_mut().editor_index = None;
        let ui = self.ui();
        ui.set_editing(false);
        ui.set_selected_profile(-1);
        ui.set_profile_dirty(false);
        ui.invoke_focus_page();
    }

    fn cancel_profile(&self) {
        if self.ui().get_profile_dirty() {
            self.ask(
                ConfirmAction::CancelEdit,
                "放弃应用的修改？",
                "Discard app changes?",
                "当前应用尚未保存。",
                "The current app has unsaved changes.",
            );
        } else {
            self.cancel_editor();
        }
    }

    fn delete_profile(&self) {
        self.ask(
            ConfirmAction::Delete,
            "删除这个应用配置？",
            "Delete this app profile?",
            "此操作移除配置及快捷键，不会卸载应用。",
            "This removes the profile and shortcut. The application remains installed.",
        );
    }

    fn toggle_profile(&self, index: i32) {
        let id = {
            let state = self.state.borrow();
            if state.toggling || !state.config.windows_enabled {
                return;
            }
            let Some(profile) = state.config.profiles.get(index as usize) else {
                return;
            };
            if !profile.enabled || profile.platform != Platform::current() {
                return;
            }
            profile.id.clone()
        };
        // A saved draft may not be lost just because the UI later unloads.
        if self.dirty() {
            self.say(
                "请先保存或取消修改，再切换窗口。",
                "Save or cancel your changes before toggling a window.",
            );
            return;
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::{
                Foundation::GetLastError, UI::WindowsAndMessaging::AllowSetForegroundWindow,
            };
            let host_pid = self.state.borrow().host_pid;
            if host_pid != 0 && unsafe { AllowSetForegroundWindow(host_pid) } == 0 {
                self.state.borrow_mut().last_error =
                    format!("AllowSetForegroundWindow({host_pid}) failed: {}", unsafe {
                        GetLastError()
                    });
            }
        }
        self.state.borrow_mut().toggling = true;
        self.hide();
        self.send(Work::Toggle(id));
    }

    fn ask(
        &self,
        action: ConfirmAction,
        zh_title: &str,
        en_title: &str,
        zh_text: &str,
        en_text: &str,
    ) {
        self.state.borrow_mut().confirmation = Some(action);
        let ui = self.ui();
        ui.set_confirm_title(if ui.get_english() { en_title } else { zh_title }.into());
        ui.set_confirm_message(if ui.get_english() { en_text } else { zh_text }.into());
        ui.set_confirm_visible(true);
        ui.invoke_focus_confirm_actions();
    }

    fn confirm(&self, yes: bool) {
        let action = self.state.borrow_mut().confirmation.take();
        self.ui().set_confirm_visible(false);
        if !yes {
            self.ui().invoke_focus_page();
            return;
        }
        match action {
            Some(ConfirmAction::Close) => {
                self.sync_settings();
                self.cancel_editor();
                self.close_manager();
            }
            Some(ConfirmAction::Edit(index)) => self.begin_edit(index),
            Some(ConfirmAction::New) => self.begin_edit(-1),
            Some(ConfirmAction::CancelEdit) => self.cancel_editor(),
            Some(ConfirmAction::ResetSettings) => {
                self.sync_settings();
                self.ui().invoke_focus_page();
            }
            Some(ConfirmAction::Delete) => {
                let index = self.state.borrow().editor_index;
                if let Some(index) = index {
                    let mut config = self.state.borrow().config.clone();
                    config.profiles.remove(index);
                    self.save(config, SaveKind::Delete);
                }
            }
            Some(ConfirmAction::Reload) => {
                self.cancel_editor();
                self.sync_settings();
                self.send(Work::Load { force: true });
            }
            Some(ConfirmAction::Import) => {
                self.sync_settings();
                self.cancel_editor();
                self.start_dialog(DialogKind::Import);
            }
            None => {}
        }
    }

    fn reload_config(&self) {
        if self.dirty() {
            self.ask(
                ConfirmAction::Reload,
                "重新载入已保存配置？",
                "Reload saved configuration?",
                "尚未保存的修改将被丢弃。",
                "Unsaved changes will be discarded.",
            );
        } else {
            self.send(Work::Load { force: true });
        }
    }

    fn start_dialog(&self, kind: DialogKind) {
        if self.state.borrow().dialog_open {
            return;
        }
        self.state.borrow_mut().dialog_open = true;
        self.ui().set_dialog_busy(true);
        let dialog = self
            .ui()
            .window()
            .with_winit_window(|window| rfd::FileDialog::new().set_parent(window))
            .unwrap_or_default();
        self.send(Work::Dialog(kind, dialog));
    }

    fn import_config(&self) {
        if self.dirty() {
            self.ask(
                ConfirmAction::Import,
                "先放弃尚未保存的修改？",
                "Discard unsaved changes first?",
                "导入基于当前已保存配置，尚未保存的修改将被丢弃。",
                "Import uses the saved configuration and discards unsaved edits.",
            );
        } else {
            self.start_dialog(DialogKind::Import);
        }
    }

    fn cancel_import(&self) {
        self.state.borrow_mut().imported = None;
        self.ui().set_import_visible(false);
        self.ui().invoke_focus_page();
    }

    fn apply_import(&self, replace: bool) {
        let result = {
            let state = self.state.borrow();
            state
                .imported
                .as_ref()
                .map(|imported| imported.apply(&state.config, replace))
        };
        match result {
            Some(Ok(config)) => self.save(config, SaveKind::Import),
            Some(Err(error)) => self.error(error),
            None => {}
        }
    }

    fn refresh_status(&self) {
        if self.ui().get_diagnostic_loading() {
            return;
        }
        self.ui().set_diagnostic_loading(true);
        self.send(Work::Status);
    }

    fn receive_status(&self, status: HostStatus) {
        {
            let mut state = self.state.borrow_mut();
            state.host_pid = status.process_id;
            state.host_status = Some(status);
        }
        self.ui().set_diagnostic_loaded(true);
        self.sync_diagnostics();
    }

    fn sync_diagnostics(&self) {
        let ui = self.ui();
        let state = self.state.borrow();
        ui.set_diagnostic_error(state.last_error.clone().into());
        let Some(status) = &state.host_status else {
            return;
        };
        ui.set_diagnostic_window_control(status.window_control);
        ui.set_diagnostic_hotkeys(status.hotkeys);
        ui.set_diagnostic_tray(status.tray);
        ui.set_diagnostic_backend(status.backend.clone().into());
        ui.set_diagnostic_platform(platform_name(Platform::current()).into());
        ui.set_diagnostic_last_action(status.last_action.clone().into());
        ui.set_diagnostic_warnings(status.warnings.join("\n").into());
        let availability = |value| match (ui.get_english(), value) {
            (true, true) => "Available",
            (true, false) => "Unavailable",
            (false, true) => "可用",
            (false, false) => "不可用",
        };
        let text = if ui.get_english() {
            format!(
                "System: {}\nBackend: {}\nHost process: {}\nWindow control: {}\nGlobal shortcuts: {}\nSystem tray: {}\nConfiguration revision: {}\n\nLast action:\n{}\n\nNotes:\n{}\n\nLast UI error:\n{}",
                platform_name(Platform::current()),
                status.backend,
                state.host_pid,
                availability(status.window_control),
                availability(status.hotkeys),
                availability(status.tray),
                state.revision,
                status.last_action,
                status.warnings.join("\n"),
                state.last_error
            )
        } else {
            format!(
                "系统：{}\n窗口后端：{}\n常驻进程：{}\n窗口控制：{}\n全局快捷键：{}\n系统托盘：{}\n配置修订号：{}\n\n最近操作：\n{}\n\n提示：\n{}\n\n最近的界面错误：\n{}",
                platform_name(Platform::current()),
                status.backend,
                state.host_pid,
                availability(status.window_control),
                availability(status.hotkeys),
                availability(status.tray),
                state.revision,
                status.last_action,
                status.warnings.join("\n"),
                state.last_error
            )
        };
        ui.set_diagnostics(text.into());
    }

    fn handle(&self, event: Event) {
        let ui = self.ui();
        match event {
            Event::Loaded { response, force } => match response {
                Ok(response) => {
                    if let Some(status) = response.status { self.receive_status(status); }
                    let Some(config) = response.config else { self.error("宿主未返回配置 / Host returned no configuration"); return; };
                    if ui.get_saving() { return; }
                    // A forced reload discards the old draft before dispatch.
                    // Never discard a new draft entered while IPC was in flight.
                    if self.dirty() {
                        if response.revision != self.state.borrow().revision { self.error("配置已在外部更改，请重新载入后再保存 / Configuration changed externally; reload before saving"); }
                        if ui.get_page() == 0 || self.state.borrow().launcher_visible { self.refresh_projects(); }
                        return;
                    }
                    let changed = response.revision != self.state.borrow().revision;
                    {
                        let mut state = self.state.borrow_mut();
                        state.config = config; state.revision = response.revision;
                    }
                    ui.set_loaded(true);
                    self.sync_settings(); self.sync_profiles();
                    if force || changed { self.cancel_editor(); }
                    self.say("已连接常驻服务。", "Connected to the resident host.");
                    if ui.get_page() == 0 || self.state.borrow().launcher_visible { self.refresh_projects(); }
                    self.sync_launcher();
                    if self.state.borrow().launcher_visible { self.launcher().invoke_focus_search(); }
                }
                Err(error) => self.error(format!("无法连接常驻服务，请先运行 sean-toolbox / Cannot connect to host; start sean-toolbox first. {error}")),
            },
            Event::Saved { response, config, kind } => {
                ui.set_saving(false);
                match response {
                    Ok(response) => {
                        {
                            let mut state = self.state.borrow_mut();
                            state.config = response.config.unwrap_or(config); state.revision = response.revision;
                        }
                        if let Some(status) = response.status { self.receive_status(status); }
                        if matches!(kind, SaveKind::Settings | SaveKind::Import) || !ui.get_settings_dirty() { self.sync_settings(); }
                        self.sync_profiles();
                        if matches!(kind, SaveKind::Profile | SaveKind::Delete | SaveKind::Import) { self.cancel_editor(); }
                        if kind == SaveKind::Import { self.cancel_import(); }
                        self.say("配置已保存并应用。", "Configuration saved and applied.");
                        if kind == SaveKind::Settings || kind == SaveKind::Import { self.refresh_projects(); }
                    }
                    Err(error) => self.error(error),
                }
            }
            Event::Projects { generation, result, total } => {
                if generation != self.query_generation.load(Ordering::Relaxed) { return; }
                self.state.borrow_mut().rendered_generation = generation;
                ui.set_project_loading(false);
                ui.set_project_total(total.min(i32::MAX as usize) as i32);
                match result {
                    Ok(items) => {
                        ui.set_project_error(false);
                        let count = items.len();
                        let rows = items.into_iter().map(|item| ProjectRow { id: item.id.into(), name: item.name.into(), location: item.location.into(), source: item.source.into() }).collect::<Vec<_>>();
                        ui.set_projects(ModelRc::new(VecModel::from(rows)));
                        ui.set_selected_project(if count == 0 { -1 } else { 0 });
                        ui.invoke_reveal_selection();
                        let message = if count == 0 {
                            let empty = if total == 0 { if ui.get_english() { "No recent projects yet. Open a folder or workspace in VS Code, then refresh." } else { "暂无最近项目。在 VS Code 中打开文件夹或工作区后刷新。" } }
                            else if ui.get_english() { "No matching projects." } else { "没有匹配的项目。" };
                            empty.to_owned()
                        } else if ui.get_english() { format!("{count} shown · {total} recent projects") } else { format!("显示 {count} 项 · 共 {total} 个最近项目") };
                        ui.set_project_message(message.into());
                    }
                    Err(error) => { ui.set_project_error(true); ui.set_projects(ModelRc::default()); ui.set_selected_project(-1); ui.set_project_message(error.clone().into()); self.error(error); }
                }
            }
            Event::Opened(result) => {
                ui.set_project_opening(false);
                let target = self.state.borrow_mut().project_open_target.take();
                match result {
                    Ok(()) => {
                        self.say("已发送项目打开请求。", "Project open request sent.");
                        // Opening may be slow. Do not hide a different page or
                        // a newer launcher session, or unload management drafts.
                        if let Some(ProjectOpenTarget::Launcher(session)) = target {
                            let state = self.state.borrow();
                            let current = state.launcher_visible && state.launcher_session == session;
                            drop(state);
                            if current { self.hide_launcher(); }
                        } else if target == Some(ProjectOpenTarget::Manager)
                            && ui.get_page() == 0 && !self.dirty() && !ui.get_confirm_visible()
                            && !ui.get_import_visible() && !ui.get_dialog_busy() && !ui.get_saving() {
                            self.hide();
                        }
                    },
                    Err(error) => self.error(error),
                }
                self.schedule_unload();
            }
            Event::Toggled(result) => {
                self.state.borrow_mut().toggling = false;
                match result {
                    Ok(response) => { ui.set_status_message(response.message.into()); ui.set_status_error(false); self.schedule_unload(); }
                    Err(error) => { let _ = self.show(UiPage::Windows, false); self.error(error); }
                }
            }
            Event::Status(result) => {
                ui.set_diagnostic_loading(false);
                match result {
                    Ok(response) => {
                        if let Some(status) = response.status { self.receive_status(status); }
                        else {
                            ui.set_diagnostic_loaded(false);
                            self.error("宿主未返回状态 / Host returned no status");
                        }
                    }
                    Err(error) => {
                        ui.set_diagnostic_loaded(false);
                        self.error(error);
                    }
                }
            }
            Event::PathPicked { kind, path } => {
                self.state.borrow_mut().dialog_open = false;
                ui.set_dialog_busy(false);
                match path {
                    Ok(Some(path)) => match kind {
                        0 => { ui.set_launch_executable(path.into()); ui.set_profile_dirty(true); }
                        1 => { ui.set_launch_cwd(path.into()); ui.set_profile_dirty(true); }
                        _ => { ui.set_setting_code_path(path.into()); ui.set_settings_dirty(true); }
                    },
                    Ok(None) => {}, Err(error) => self.error(error),
                }
            }
            Event::Imported(result) => {
                self.state.borrow_mut().dialog_open = false;
                ui.set_dialog_busy(false);
                match result {
                    Ok(Some(imported)) => {
                        self.say("配置已读取，请预览后应用。", "Configuration loaded; review before applying.");
                        ui.set_import_is_json(imported.config.is_some());
                        ui.set_import_summary(imported.summary(ui.get_english()).into());
                        self.state.borrow_mut().imported = Some(imported);
                        ui.set_import_visible(true);
                        ui.invoke_focus_import_actions();
                    }
                    Ok(None) => {}, Err(error) => self.error(error),
                }
            }
            Event::Exported(result) => {
                self.state.borrow_mut().dialog_open = false;
                ui.set_dialog_busy(false);
                match result { Ok(Some(path)) => { self.say("配置已导出。", "Configuration exported."); ui.set_status_message(format!("{} {}", if ui.get_english() { "Exported:" } else { "已导出：" }, path.display()).into()); }, Ok(None) => {}, Err(error) => self.error(error) }
            }
            Event::LinkOpened(result) => { if let Err(error) = result { self.error(error); } }
            Event::ServerError(error) => self.error(format!("界面唤起服务失败 / UI activation service failed: {error}")),
            Event::Ready { reply } => {
                let response = if self.state.borrow().exit_requested { Response::error("UI is closing") } else { Response::success("ready") };
                let _ = reply.send(response);
            }
            Event::Show { page, reply } => {
                let response = match self.show(page, true) { Ok(()) => Response::success("shown"), Err(error) => Response::error(error) };
                let _ = reply.send(response);
            }
        }
        self.sync_launcher();
    }
}

fn arguments() -> Result<(UiPage, String), String> {
    let mut page = UiPage::Projects;
    let mut endpoint = "sean-toolbox-ui-v1".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--page" => {
                page = match args.next().as_deref() {
                    Some("projects") => UiPage::Projects,
                    Some("windows") => UiPage::Windows,
                    Some("settings") => UiPage::Settings,
                    Some("diagnostics") => UiPage::Diagnostics,
                    _ => return Err("--page expects projects/windows/settings/diagnostics".into()),
                }
            }
            "--endpoint" => endpoint = args.next().ok_or("--endpoint expects an identifier")?,
            "--help" | "-h" => {
                println!(
                    "sean-toolbox-ui [--page projects|windows|settings|diagnostics] [--endpoint ID]\nStart sean-toolbox to launch the resident host first."
                );
                std::process::exit(0);
            }
            _ => return Err(format!("Unknown argument: {arg}")),
        }
    }
    Ok((page, endpoint))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (page, endpoint) = arguments()?;
    worker::init_dialog_logging();
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    let ui = AppWindow::new()?;
    let launcher = LauncherWindow::new()?;
    ui.set_version_label(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    let (send, receive) = mpsc::channel();
    let query_generation = Arc::new(AtomicU64::new(0));
    let controller = Rc::new(Controller {
        ui: ui.as_weak(),
        launcher: launcher.as_weak(),
        work: send,
        query_generation: query_generation.clone(),
        idle_timer: Timer::default(),
        state: RefCell::new(State {
            config: Config::default(),
            revision: 0,
            editor: None,
            editor_index: None,
            candidate: 0,
            imported: None,
            confirmation: None,
            dialog_open: false,
            visible: false,
            release_requested: false,
            exit_requested: false,
            launcher_visible: false,
            launcher_had_focus: false,
            launcher_session: 0,
            launcher_placement_pending: false,
            launcher_placement_attempts: 0,
            launcher_display: None,
            project_open_target: None,
            toggling: false,
            rendered_generation: 0,
            host_pid: 0,
            host_status: None,
            last_error: String::new(),
        }),
    });
    APP.with(|app| *app.borrow_mut() = Some(controller.clone()));
    ui.on_navigate(|page| with_controller(|c| c.navigate(page)));
    ui.on_dismiss(|| with_controller(Controller::dismiss));
    ui.on_query_changed(|query| with_controller(|c| c.query_changed(query.into())));
    ui.on_refresh_projects(|| with_controller(Controller::refresh_projects));
    ui.on_open_project(|index| with_controller(|c| c.open_project(index)));
    ui.on_move_project(|delta| with_controller(|c| c.move_project(delta)));
    ui.on_edit_profile(|index| with_controller(|c| c.edit_profile(index)));
    ui.on_new_profile(|| with_controller(Controller::new_profile));
    ui.on_delete_profile(|| with_controller(Controller::delete_profile));
    ui.on_save_profile(|| with_controller(Controller::save_profile));
    ui.on_cancel_profile(|| with_controller(Controller::cancel_profile));
    ui.on_profile_changed(|| with_controller(Controller::profile_changed));
    ui.on_toggle_profile(|index| with_controller(|c| c.toggle_profile(index)));
    ui.on_candidate_selected(|index| with_controller(|c| c.select_candidate(index)));
    ui.on_add_candidate(|| with_controller(Controller::add_candidate));
    ui.on_remove_candidate(|| with_controller(Controller::remove_candidate));
    ui.on_browse_path(|kind| with_controller(|c| c.start_dialog(DialogKind::Browse(kind))));
    ui.on_settings_changed(|| with_controller(Controller::settings_changed));
    ui.on_save_settings(|| with_controller(Controller::save_settings));
    ui.on_reset_settings(|| with_controller(Controller::reset_settings));
    ui.on_reload_config(|| with_controller(Controller::reload_config));
    ui.on_import_config(|| with_controller(Controller::import_config));
    ui.on_export_config(|| with_controller(|c| c.start_dialog(DialogKind::Export)));
    ui.on_apply_import(|replace| with_controller(|c| c.apply_import(replace)));
    ui.on_cancel_import(|| with_controller(Controller::cancel_import));
    ui.on_refresh_status(|| with_controller(Controller::refresh_status));
    ui.on_confirm_action(|yes| with_controller(|c| c.confirm(yes)));
    ui.on_open_link(|index| with_controller(|c| c.send(Work::Link(index))));
    ui.window().on_close_requested(|| {
        with_controller(Controller::dismiss);
        slint::CloseRequestResponse::KeepWindowShown
    });
    launcher.on_query_changed(|query| with_controller(|c| c.query_changed(query.into())));
    launcher.on_refresh_projects(|| with_controller(Controller::refresh_projects));
    launcher.on_select_project(|index| with_controller(|c| c.select_launcher_project(index)));
    launcher.on_open_project(|index| with_controller(|c| c.open_launcher_project(index)));
    launcher.on_move_project(|delta| with_controller(|c| c.move_project(delta)));
    launcher.on_dismiss(|| with_controller(Controller::close_launcher));
    launcher.on_open_settings(|| with_controller(Controller::open_launcher_settings));
    launcher.window().on_close_requested(|| {
        with_controller(Controller::close_launcher);
        slint::CloseRequestResponse::KeepWindowShown
    });
    launcher.window().on_winit_window_event(|_window, event| {
        match event {
            winit::event::WindowEvent::Focused(focused) => {
                // macOS can report a stale Focused payload (winit#4371).
                // Use the same native state query as Slint's own backend there.
                #[cfg(target_os = "macos")]
                let focused = _window
                    .with_winit_window(|window| window.has_focus())
                    .unwrap_or(*focused);
                #[cfg(not(target_os = "macos"))]
                let focused = *focused;
                if focused {
                    with_controller(|c| c.launcher_focus_changed(true));
                } else {
                    // Let the window manager finish focus/property updates
                    // before distinguishing an X11 shortcut grab from blur.
                    with_controller(Controller::queue_launcher_focus_loss);
                }
            }
            winit::event::WindowEvent::Resized(_) => {
                with_controller(|controller| {
                    if controller.state.borrow().launcher_placement_pending {
                        controller.queue_launcher_placement();
                    }
                });
            }
            winit::event::WindowEvent::ScaleFactorChanged { .. } => {
                with_controller(Controller::queue_launcher_placement);
            }
            _ => {}
        }
        EventResult::Propagate
    });
    controller.show(page, false)?;
    std::thread::Builder::new()
        .name("toolbox-ui-worker".into())
        .spawn(move || worker::run(receive, query_generation))?;
    let server_stopping = Arc::new(AtomicBool::new(false));
    let serve_stopping = server_stopping.clone();
    let serve_endpoint = endpoint.clone();
    let activation = std::thread::Builder::new()
        .name("toolbox-ui-activation".into())
        .spawn(move || worker::serve(serve_endpoint, serve_stopping))?;
    controller.send(Work::Load { force: false });
    slint::run_event_loop_until_quit()?;
    APP.with(|app| *app.borrow_mut() = None);
    server_stopping.store(true, Ordering::Relaxed);
    let cleanup = std::thread::spawn(move || {
        worker::stop_server(&endpoint);
        let _ = activation.join();
    });
    let _ = cleanup.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};

    thread_local! {
        static TEST_WINDOWS: RefCell<Vec<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(Vec::new()) };
    }

    struct TestPlatform;

    impl slint::platform::Platform for TestPlatform {
        fn create_window_adapter(
            &self,
        ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
            let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
            TEST_WINDOWS.with(|windows| windows.borrow_mut().push(window.clone()));
            Ok(window)
        }
    }

    fn with_test_ui(
        test: impl FnOnce(&AppWindow, &Controller, &mpsc::Receiver<Work>) + Send + 'static,
    ) {
        // Each test owns a Slint context, software window and disconnected worker.
        // No native desktop, host process, configuration or user data is accessed.
        std::thread::spawn(move || {
            slint::platform::set_platform(Box::new(TestPlatform)).unwrap();
            let ui = AppWindow::new().unwrap();
            let launcher = LauncherWindow::new().unwrap();
            let (send, receive) = mpsc::channel();
            let controller = Controller {
                ui: ui.as_weak(),
                launcher: launcher.as_weak(),
                work: send,
                query_generation: Arc::new(AtomicU64::new(0)),
                idle_timer: Timer::default(),
                state: RefCell::new(State {
                    config: Config::default(),
                    revision: 1,
                    editor: None,
                    editor_index: None,
                    candidate: 0,
                    imported: None,
                    confirmation: None,
                    dialog_open: false,
                    visible: true,
                    release_requested: false,
                    exit_requested: false,
                    launcher_visible: false,
                    launcher_had_focus: false,
                    launcher_session: 0,
                    launcher_placement_pending: false,
                    launcher_placement_attempts: 0,
                    launcher_display: None,
                    project_open_target: None,
                    toggling: false,
                    rendered_generation: 0,
                    host_pid: 0,
                    host_status: None,
                    last_error: String::new(),
                }),
            };
            ui.set_loaded(true);
            controller.sync_settings();
            controller.sync_profiles();
            test(&ui, &controller, &receive);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn theme_preview_reaches_both_surfaces_and_discard_preserves_app_drafts() {
        with_test_ui(|ui, controller, _| {
            controller.state.borrow_mut().config.theme = ThemePreference::Light;
            controller.sync_settings();
            controller.begin_edit(0);
            ui.set_launch_args("[unfinished".into());
            controller.profile_changed();

            ui.set_setting_theme(2);
            controller.settings_changed();
            assert!(ui.global::<Theme>().get_dark());
            assert!(controller.launcher().global::<Theme>().get_dark());
            assert!(ui.get_settings_dirty());
            assert_eq!(
                controller.state.borrow().config.theme,
                ThemePreference::Light
            );
            assert_eq!(
                controller.settings_config().unwrap().theme,
                ThemePreference::Dark
            );

            controller.reset_settings();
            controller.confirm(true);
            assert_eq!(ui.get_setting_theme(), 1);
            assert!(!ui.global::<Theme>().get_dark());
            assert!(!controller.launcher().global::<Theme>().get_dark());
            assert!(!ui.get_settings_dirty());
            assert!(ui.get_profile_dirty());
            assert_eq!(ui.get_launch_args(), "[unfinished");

            ui.set_setting_theme(0);
            controller.settings_changed();
            assert_eq!(
                ui.global::<Palette>().get_color_scheme(),
                slint::language::ColorScheme::Unknown
            );
            assert_eq!(
                controller.launcher().global::<Palette>().get_color_scheme(),
                slint::language::ColorScheme::Unknown
            );
        });
    }

    #[test]
    fn saving_quake_options_preserves_launch_arguments_and_pending_theme() {
        with_test_ui(|ui, controller, receive| {
            controller.begin_edit(0);
            ui.set_form_platform(0);
            ui.set_form_quake_enabled(true);
            ui.set_form_quake_edge(3);
            ui.set_form_quake_alignment(2);
            ui.set_form_quake_width(40);
            ui.set_form_quake_height(75);
            ui.set_form_quake_monitor(1);
            ui.set_form_quake_topmost(true);
            ui.set_form_quake_auto_hide(true);
            ui.set_launch_args(r#"["literal path with spaces", "$(not-a-shell)"]"#.into());
            controller.profile_changed();
            ui.set_setting_theme(2);
            controller.settings_changed();
            controller.save_profile();

            let Work::Save { config, kind, .. } = receive.try_recv().unwrap() else {
                panic!("Expected a validated app save");
            };
            assert_eq!(kind, SaveKind::Profile);
            assert_eq!(config.theme, ThemePreference::System);
            let profile = &config.profiles[0];
            assert!(profile.quake.enabled);
            assert_eq!(profile.quake.edge, toolbox_core::QuakeEdge::Right);
            assert_eq!(profile.quake.alignment, toolbox_core::QuakeAlignment::End);
            assert_eq!(
                (profile.quake.width_percent, profile.quake.height_percent),
                (40, 75)
            );
            assert_eq!(
                profile.quake.monitor,
                toolbox_core::QuakeMonitor::Foreground
            );
            assert!(profile.quake.always_on_top);
            assert!(profile.quake.hide_on_focus_loss);
            assert_eq!(
                profile.launch[0].args,
                ["literal path with spaces", "$(not-a-shell)"]
            );
            controller.handle(Event::Saved {
                response: Ok(Response {
                    ok: true,
                    revision: 2,
                    ..Response::default()
                }),
                config,
                kind,
            });
            assert!(ui.get_settings_dirty());
            assert!(ui.global::<Theme>().get_dark());
            assert_eq!(ui.get_form_quake_width(), 40);
            assert!(!ui.get_profile_dirty());
        });
    }

    #[test]
    fn unsupported_quake_controls_cannot_leak_across_platform_edits() {
        with_test_ui(|ui, controller, receive| {
            controller.begin_edit(0);
            ui.set_form_platform(0);
            ui.set_form_quake_enabled(true);
            ui.set_form_quake_topmost(true);
            ui.set_form_quake_auto_hide(true);
            ui.set_form_platform(1);
            controller.profile_changed();
            assert!(!ui.get_form_quake_topmost());
            assert!(!ui.get_form_quake_auto_hide());
            ui.set_form_quake_width(0);
            controller.save_profile();
            assert!(ui.get_status_error());
            assert!(receive.try_recv().is_err());
            assert!(ui.get_profile_dirty());
        });
    }

    #[test]
    fn explicit_close_bypasses_cache_but_never_unloads_a_visible_or_busy_surface() {
        with_test_ui(|ui, controller, _| {
            controller.state.borrow_mut().config.ui_idle_seconds = 60;
            controller.state.borrow_mut().visible = false;
            controller.state.borrow_mut().launcher_visible = true;
            controller.hide_launcher();
            assert_eq!(controller.unload_delay(), Some(Duration::from_secs(60)));

            controller.state.borrow_mut().launcher_visible = true;
            controller.close_launcher();
            assert_eq!(controller.unload_delay(), Some(Duration::ZERO));
            controller.state.borrow_mut().visible = true;
            assert_eq!(controller.unload_delay(), None);
            controller.state.borrow_mut().visible = false;
            ui.set_project_opening(true);
            assert_eq!(controller.unload_delay(), None);
            ui.set_project_opening(false);
            ui.set_saving(true);
            assert_eq!(controller.unload_delay(), None);
            ui.set_saving(false);
            controller.state.borrow_mut().dialog_open = true;
            assert_eq!(controller.unload_delay(), None);
            controller.state.borrow_mut().dialog_open = false;
            ui.set_profile_dirty(true);
            assert_eq!(controller.unload_delay(), None);
            ui.set_profile_dirty(false);
            assert_eq!(controller.unload_delay(), Some(Duration::ZERO));
            controller.show(UiPage::Settings, false).unwrap();
            assert_eq!(controller.unload_delay(), None);
            assert!(!controller.state.borrow().release_requested);
        });
    }

    #[test]
    fn cancelling_close_keeps_drafts_and_confirming_releases_memory() {
        with_test_ui(|ui, controller, _| {
            controller.state.borrow_mut().config.ui_idle_seconds = 60;
            controller.begin_edit(0);
            ui.set_form_name("Unsaved application".into());
            controller.profile_changed();
            controller.dismiss();
            assert!(ui.get_confirm_visible());
            controller.confirm(false);
            assert!(controller.state.borrow().visible);
            assert!(ui.get_profile_dirty());
            assert_eq!(controller.unload_delay(), None);
            controller.dismiss();
            controller.confirm(true);
            assert!(!ui.get_profile_dirty());
            assert!(!controller.state.borrow().visible);
            assert_eq!(controller.unload_delay(), Some(Duration::ZERO));
        });
    }

    #[test]
    fn committed_exit_rejects_readiness_and_activation_instead_of_losing_a_summon() {
        with_test_ui(|_ui, controller, receive| {
            controller.state.borrow_mut().visible = false;
            controller.state.borrow_mut().exit_requested = true;
            let (reply, ready) = mpsc::channel();
            controller.handle(Event::Ready { reply });
            let response = ready.recv().unwrap();
            assert!(!response.ok);
            assert_eq!(response.message, "UI is closing");
            for page in [UiPage::Projects, UiPage::Settings] {
                let (reply, shown) = mpsc::channel();
                controller.handle(Event::Show { page, reply });
                let response = shown.recv().unwrap();
                assert!(!response.ok);
                assert_eq!(response.message, "UI is closing");
            }
            assert!(!controller.state.borrow().visible);
            assert!(!controller.state.borrow().launcher_visible);
            assert!(receive.try_recv().is_err());
        });
    }

    #[test]
    fn navigating_from_another_start_page_discovers_projects() {
        with_test_ui(|ui, controller, receive| {
            ui.set_page(1);
            controller.navigate(0);
            assert_eq!(ui.get_page(), 0);
            assert!(ui.get_project_loading());
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Discover { generation: 1, query, .. } if query.is_empty()
            ));
            controller.handle(Event::Projects {
                generation: 1,
                result: Ok(vec![]),
                total: 0,
            });
            assert!(!ui.get_project_loading());
            assert!(!ui.get_project_error());
            assert_eq!(ui.get_selected_project(), -1);
        });
    }

    #[test]
    fn late_project_open_does_not_hide_new_drafts_or_other_pages() {
        with_test_ui(|ui, controller, _| {
            ui.set_page(0);
            ui.set_project_opening(true);
            controller.state.borrow_mut().project_open_target = Some(ProjectOpenTarget::Manager);
            ui.set_setting_hotkey("Ctrl+Alt+P".into());
            controller.settings_changed();
            controller.handle(Event::Opened(Ok(())));
            assert!(controller.state.borrow().visible);
            assert!(ui.get_settings_dirty());
            assert_eq!(ui.get_setting_hotkey(), "Ctrl+Alt+P");
            assert!(!ui.get_project_opening());

            controller.sync_settings();
            ui.set_page(1);
            ui.set_project_opening(true);
            controller.state.borrow_mut().project_open_target = Some(ProjectOpenTarget::Manager);
            controller.handle(Event::Opened(Ok(())));
            assert!(controller.state.borrow().visible);
            assert_eq!(ui.get_page(), 1);
        });
    }

    #[test]
    fn reload_does_not_erase_edits_started_while_the_request_is_pending() {
        with_test_ui(|ui, controller, receive| {
            ui.set_page(2);
            controller.reload_config();
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Load { force: true }
            ));
            ui.set_setting_idle("120".into());
            controller.settings_changed();
            controller.handle(Event::Loaded {
                response: Ok(Response {
                    ok: true,
                    config: Some(Config {
                        ui_idle_seconds: 30,
                        ..Config::default()
                    }),
                    revision: 2,
                    ..Response::default()
                }),
                force: true,
            });
            assert_eq!(ui.get_setting_idle(), "120");
            assert!(ui.get_settings_dirty());
            assert!(ui.get_status_error());
            assert_eq!(controller.state.borrow().revision, 1);
        });
    }

    #[test]
    fn resetting_settings_preserves_an_application_and_its_unparsed_candidate() {
        with_test_ui(|ui, controller, _| {
            ui.set_page(1);
            controller.begin_edit(0);
            ui.set_form_name("My terminal draft".into());
            ui.set_launch_args("[".into());
            controller.profile_changed();
            ui.set_setting_language(1);
            ui.set_setting_windows(false);
            controller.settings_changed();
            assert!(ui.get_windows_enabled());
            assert!(!ui.get_setting_windows());
            controller.reset_settings();
            assert!(ui.get_confirm_visible());
            controller.confirm(true);
            assert!(!ui.get_settings_dirty());
            assert!(ui.get_setting_windows());
            assert!(!ui.get_english());
            assert!(ui.get_profile_dirty());
            assert_eq!(ui.get_form_name(), "My terminal draft");
            assert_eq!(ui.get_launch_args(), "[");
            assert!(ui.get_editing());

            controller.edit_profile(0);
            assert!(!ui.get_confirm_visible());
            assert_eq!(ui.get_form_name(), "My terminal draft");
        });
    }

    #[test]
    fn failed_status_refresh_invalidates_previous_capabilities() {
        with_test_ui(|ui, controller, receive| {
            controller.receive_status(HostStatus {
                process_id: 42,
                backend: "test backend".into(),
                window_control: true,
                hotkeys: true,
                tray: true,
                ..HostStatus::default()
            });
            assert!(ui.get_diagnostic_loaded());
            assert!(ui.get_diagnostic_hotkeys());
            controller.refresh_status();
            controller.refresh_status();
            assert!(matches!(receive.try_recv().unwrap(), Work::Status));
            assert!(receive.try_recv().is_err());
            assert!(ui.get_diagnostic_loading());
            controller.handle(Event::Status(Err("Host disconnected".into())));
            assert!(!ui.get_diagnostic_loaded());
            assert!(!ui.get_diagnostic_loading());
            assert_eq!(ui.get_diagnostic_error(), "Host disconnected");
        });
    }

    fn receive_project(controller: &Controller, id: &str) {
        use toolbox_projects::{ProjectId, ProjectKind, ProjectTarget, ProviderId, RecentProject};
        controller.handle(Event::Projects {
            generation: controller.query_generation.load(Ordering::Relaxed),
            result: Ok(vec![
                RecentProject {
                    id: ProjectId(id.to_owned()),
                    provider: ProviderId("vscode".into()),
                    kind: ProjectKind::Folder,
                    target: ProjectTarget::Uri(format!("vscode-remote://ssh-remote+dev/{id}")),
                    name: id.to_owned(),
                    detail: format!("dev/{id}"),
                    recent_index: 0,
                }
                .into(),
            ]),
            total: 1,
        });
    }

    #[test]
    fn launcher_opens_projects_without_discarding_management_drafts() {
        with_test_ui(|ui, controller, receive| {
            ui.set_page(1);
            controller.begin_edit(0);
            ui.set_form_name("Keep this application draft".into());
            ui.set_launch_args("[".into());
            controller.profile_changed();
            ui.set_setting_idle("120".into());
            controller.settings_changed();
            ui.set_query("old query".into());

            controller.show(UiPage::Projects, false).unwrap();
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Discover { query, .. } if query.is_empty())
            );
            assert_eq!(ui.get_page(), 1);
            assert_eq!(controller.launcher().get_query(), "");
            assert!(controller.state.borrow().visible);
            assert!(controller.state.borrow().launcher_visible);
            assert!(!ui.get_confirm_visible());

            receive_project(controller, "api-service");
            controller.open_launcher_project(0);
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Open { id, .. } if id == "api-service")
            );
            controller.handle(Event::Opened(Ok(())));
            assert!(!controller.state.borrow().launcher_visible);
            assert!(controller.state.borrow().visible);
            assert!(ui.get_profile_dirty());
            assert!(ui.get_settings_dirty());
            assert_eq!(ui.get_form_name(), "Keep this application draft");
            assert_eq!(ui.get_launch_args(), "[");
            assert_eq!(ui.get_setting_idle(), "120");
            assert_eq!(ui.get_page(), 1);
            assert!(!ui.get_confirm_visible());
        });
    }

    #[test]
    fn cold_typing_and_readiness_acknowledgements_preserve_the_launcher_query() {
        with_test_ui(|ui, controller, receive| {
            ui.set_loaded(false);
            controller.show(UiPage::Projects, false).unwrap();
            let session = controller.state.borrow().launcher_session;
            controller.query_changed("pair room".into());
            assert!(receive.try_recv().is_err());
            let (reply, ready) = mpsc::channel();
            controller.handle(Event::Ready { reply });
            assert!(ready.recv().unwrap().ok);
            assert_eq!(controller.state.borrow().launcher_session, session);
            assert_eq!(controller.launcher().get_query(), "pair room");

            controller.handle(Event::Loaded {
                response: Ok(Response {
                    ok: true,
                    config: Some(Config::default()),
                    revision: 1,
                    ..Response::default()
                }),
                force: false,
            });
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Discover { query, .. } if query == "pair room")
            );
            receive_project(controller, "pair-room");
            let (reply, ready) = mpsc::channel();
            controller.handle(Event::Ready { reply });
            assert!(ready.recv().unwrap().ok);
            assert_eq!(controller.launcher().get_query(), "pair room");
            assert_eq!(controller.launcher().get_selected_project(), 0);

            let (reply, shown) = mpsc::channel();
            controller.handle(Event::Show {
                page: UiPage::Projects,
                reply,
            });
            assert!(shown.recv().unwrap().ok);
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Load { force: false }
            ));
            assert_eq!(controller.launcher().get_query(), "");
            assert_eq!(controller.state.borrow().launcher_session, session + 1);
        });
    }

    #[test]
    fn launcher_open_failure_remains_visible_and_can_be_retried() {
        with_test_ui(|_, controller, receive| {
            controller.show(UiPage::Projects, false).unwrap();
            let _ = receive.try_recv().unwrap();
            receive_project(controller, "missing-cli");
            controller.open_launcher_project(0);
            assert!(matches!(receive.try_recv().unwrap(), Work::Open { .. }));
            controller.handle(Event::Opened(
                Err("VS Code executable was not found".into()),
            ));
            assert!(controller.state.borrow().launcher_visible);
            assert!(!controller.launcher().get_project_opening());
            assert!(controller.launcher().get_status_error());
            assert_eq!(
                controller.launcher().get_status_message(),
                "VS Code executable was not found"
            );
            controller.open_launcher_project(0);
            assert!(matches!(receive.try_recv().unwrap(), Work::Open { .. }));
        });
    }

    #[test]
    fn old_project_open_completion_cannot_hide_a_new_launcher_session() {
        with_test_ui(|ui, controller, receive| {
            ui.set_page(2);
            controller.show(UiPage::Projects, false).unwrap();
            let _ = receive.try_recv().unwrap();
            receive_project(controller, "first-project");
            controller.open_launcher_project(0);
            assert!(matches!(receive.try_recv().unwrap(), Work::Open { .. }));
            controller.hide_launcher();
            controller.show(UiPage::Projects, false).unwrap();
            assert!(matches!(receive.try_recv().unwrap(), Work::Discover { .. }));
            controller.handle(Event::Opened(Ok(())));
            assert!(controller.state.borrow().launcher_visible);
            assert!(controller.state.borrow().visible);
            assert_eq!(ui.get_page(), 2);
            assert!(!ui.get_project_opening());
        });
    }

    #[test]
    fn launcher_focus_loss_dismisses_only_after_focus_was_acquired() {
        with_test_ui(|ui, controller, _| {
            ui.set_page(2);
            ui.set_setting_idle("180".into());
            controller.settings_changed();
            controller.show(UiPage::Projects, false).unwrap();
            controller.launcher_focus_changed(false);
            assert!(controller.state.borrow().launcher_visible);
            controller.launcher_focus_changed(true);
            controller.launcher_focus_changed(false);
            assert!(!controller.state.borrow().launcher_visible);
            assert!(controller.state.borrow().visible);
            assert!(ui.get_settings_dirty());
            assert_eq!(ui.get_setting_idle(), "180");
            assert!(!ui.get_confirm_visible());
        });
    }

    #[test]
    fn repeating_summon_does_not_disarm_focus_loss_dismissal() {
        with_test_ui(|_, controller, _| {
            controller.show(UiPage::Projects, false).unwrap();
            controller.launcher_focus_changed(true);
            controller.show(UiPage::Projects, false).unwrap();
            assert!(controller.state.borrow().launcher_visible);
            assert!(controller.state.borrow().launcher_had_focus);
            controller.launcher_focus_changed(false);
            assert!(!controller.state.borrow().launcher_visible);
        });
    }

    #[test]
    fn launcher_does_not_open_stale_or_empty_search_results() {
        with_test_ui(|ui, controller, receive| {
            controller.show(UiPage::Projects, false).unwrap();
            let _ = receive.try_recv().unwrap();
            receive_project(controller, "old-project");
            let stale_generation = controller.query_generation.load(Ordering::Relaxed);
            controller.query_changed("nothing matches".into());
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Search { query, .. } if query == "nothing matches")
            );
            controller.open_launcher_project(0);
            assert!(receive.try_recv().is_err());
            controller.handle(Event::Projects {
                generation: stale_generation,
                result: Ok(vec![]),
                total: 0,
            });
            assert!(ui.get_project_loading());
            controller.handle(Event::Projects {
                generation: controller.query_generation.load(Ordering::Relaxed),
                result: Ok(vec![]),
                total: 1,
            });
            assert_eq!(controller.launcher().get_selected_project(), -1);
            assert_eq!(controller.launcher().get_projects().row_count(), 0);
            controller.open_launcher_project(0);
            controller.open_launcher_project(-1);
            assert!(receive.try_recv().is_err());
        });
    }

    #[test]
    fn launcher_does_not_replace_an_active_configuration_confirmation() {
        with_test_ui(|ui, controller, receive| {
            ui.set_page(1);
            controller.begin_edit(0);
            controller.profile_changed();
            controller.cancel_profile();
            assert!(ui.get_confirm_visible());
            controller.show(UiPage::Projects, true).unwrap();
            assert!(!controller.state.borrow().launcher_visible);
            assert!(controller.state.borrow().visible);
            assert_eq!(ui.get_page(), 1);
            assert!(ui.get_confirm_visible());
            assert!(receive.try_recv().is_err());
        });
    }

    #[test]
    fn launcher_centering_respects_negative_display_coordinates_and_work_area() {
        let area = DisplayArea {
            x: -1920,
            y: 40,
            width: 1920,
            height: 1040,
            monitor_origin: (-1920, 0),
        };
        assert_eq!(centered_position(area, 760, 408), (-1340, 356));
        assert_eq!(centered_position(area, 1920, 1040), (-1920, 40));
        assert_eq!(centered_position(area, 3000, 2000), (-1920, 40));
    }

    #[test]
    fn action_and_navigation_buttons_accept_the_first_pointer_click() {
        with_test_ui(|ui, _, _| {
            use slint::platform::{PointerEventButton, WindowEvent};
            use std::cell::Cell;

            ui.set_english(true);
            ui.set_page(1);
            ui.window().set_size(slint::LogicalSize::new(1120.0, 780.0));
            ui.show().unwrap();

            let added = Rc::new(Cell::new(0));
            let additions = added.clone();
            ui.on_new_profile(move || additions.set(additions.get() + 1));
            let edited = Rc::new(Cell::new(-1));
            let edits = edited.clone();
            ui.on_edit_profile(move |index| edits.set(index));
            let navigated = Rc::new(Cell::new(-1));
            let navigation = navigated.clone();
            ui.on_navigate(move |page| navigation.set(page));

            let render = || {
                TEST_WINDOWS.with(|windows| {
                    let windows = windows.borrow();
                    let window = &windows[0];
                    let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(1120, 780);
                    window.draw_if_needed(|renderer| {
                        renderer.render(pixels.make_mut_slice(), 1120);
                    });
                });
            };
            let click = |x, y| {
                let position = slint::LogicalPosition::new(x, y);
                ui.window().dispatch_event(WindowEvent::PointerPressed {
                    position,
                    button: PointerEventButton::Left,
                });
                ui.window().dispatch_event(WindowEvent::PointerReleased {
                    position,
                    button: PointerEventButton::Left,
                });
            };

            // Centers observed in native UI acceptance at the default content size:
            // Add application, first profile's Edit, and the Settings navigation row.
            // Rendering first exercises real hit testing, not direct callback invocation.
            render();
            click(336.0, 129.0);
            assert_eq!(added.get(), 1, "the first mouse click must activate Add");
            click(288.0, 274.0);
            assert_eq!(edited.get(), 0, "the first mouse click must activate Edit");
            click(100.0, 621.0);
            assert_eq!(navigated.get(), 2, "navigation must accept its first click");

            ui.set_saving(true);
            render();
            click(336.0, 129.0);
            assert_eq!(added.get(), 1, "a disabled action must not fire");
        });
    }
}
