//! Render the production Slint components with explicit example data.
//!
//! Run through `python scripts/render_ui.py`. This does not start the host,
//! read user configuration, create an IPC endpoint, or launch an application.

use slint::platform::software_renderer::{RepaintBufferType, SoftwareRenderer};
use slint::platform::{
    LayoutConstraints, Platform, Renderer, WindowAdapter, WindowEvent, WindowProperties,
};
use slint::{ComponentHandle, ModelRc, PhysicalSize, Rgb8Pixel, SharedPixelBuffer, VecModel};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::error::Error;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use toolbox_core::skills::SkillDeployMode;
use toolbox_i18n::{Locale, messages};

slint::include_modules!();

#[derive(Default)]
struct CapturePlatform {
    windows: Rc<RefCell<Vec<Rc<CaptureWindow>>>>,
}

struct CaptureWindow {
    window: slint::Window,
    renderer: SoftwareRenderer,
    size: Cell<PhysicalSize>,
    constraints: Cell<Option<LayoutConstraints>>,
}

impl WindowAdapter for CaptureWindow {
    fn window(&self) -> &slint::Window {
        &self.window
    }

    fn size(&self) -> PhysicalSize {
        self.size.get()
    }

    fn set_size(&self, size: slint::WindowSize) {
        let scale = self.window.scale_factor();
        self.size.set(size.to_physical(scale));
        self.window.dispatch_event(WindowEvent::Resized {
            size: size.to_logical(scale),
        });
    }

    fn renderer(&self) -> &dyn Renderer {
        &self.renderer
    }

    fn update_window_properties(&self, properties: WindowProperties<'_>) {
        self.constraints.set(Some(properties.layout_constraints()));
    }
}

impl Platform for CapturePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        let window = Rc::new_cyclic(|weak: &Weak<CaptureWindow>| CaptureWindow {
            window: slint::Window::new(weak.clone()),
            renderer: SoftwareRenderer::new_with_repaint_buffer_type(RepaintBufferType::NewBuffer),
            size: Cell::default(),
            constraints: Cell::default(),
        });
        self.windows.borrow_mut().push(window.clone());
        Ok(window)
    }
}

fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(rows)).into()
}

fn projects() -> ModelRc<ProjectRow> {
    let defaults = ProjectRow {
        source: "VS Code".into(),
        ..Default::default()
    };
    model(
        [
            ("toolbox", "sean-toolbox", r"C:\Work\personal\sean-toolbox"),
            ("skills", "skills", r"C:\Work\personal\skills"),
            ("robot", "robot-control", r"C:\Work\robotics\robot-control"),
            (
                "robot-lab",
                "robot-control",
                r"D:\Lab\prototype\robot-control",
            ),
            (
                "remote",
                "feishu-bot",
                "vscode-remote://ssh-remote+devbox/work/feishu-bot",
            ),
            (
                "chinese",
                "中文项目 · 数据采集",
                r"D:\研发\设备与数据采集\工作区",
            ),
        ]
        .into_iter()
        .map(|(id, name, location)| ProjectRow {
            id: id.into(),
            name: name.into(),
            location: location.into(),
            // These fixture paths are shorter than the production 52-character
            // elision threshold, so the display label remains the original path.
            display_location: location.into(),
            remote: location.starts_with("vscode-remote://"),
            ..defaults.clone()
        })
        .collect(),
    )
}

fn configure_manager(
    ui: &AppWindow,
    locale: Locale,
    theme: slint::language::ColorScheme,
    scene: &str,
) {
    ui.global::<Palette>().set_color_scheme(theme);
    ui.set_loaded(true);
    ui.set_status_message("Example data · Slint software renderer".into());
    ui.set_version_label("UI fixture".into());
    ui.set_setting_language(i32::from(locale == Locale::En));
    ui.set_setting_theme(if theme == slint::language::ColorScheme::Dark {
        2
    } else {
        1
    });
    ui.set_setting_hotkey("Ctrl+Alt+Space".into());
    ui.set_setting_idle("0".into());
    ui.set_setting_projects(true);
    ui.set_setting_windows(true);
    ui.set_setting_autostart(true);
    ui.set_projects(projects());
    ui.set_project_total(6);
    ui.set_selected_project(0);
    ui.set_project_message("VS Code · 6 example projects".into());
    let profile_defaults = ProfileRow {
        platform: "Windows".into(),
        available: true,
        ..Default::default()
    };
    ui.set_profiles(model(
        [
            ("WezTerm", "F3"),
            ("VS Code", "F4"),
            ("Chrome", "F5"),
            ("Obsidian", "Ctrl+Alt+O"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (name, hotkey))| {
            let docked = index == 0 && scene == "quake";
            ProfileRow {
                source_index: index as i32,
                name: name.into(),
                hotkey: hotkey.into(),
                detail: format!(
                    "Windows · {} · {}",
                    if docked {
                        messages::profile_docked()
                    } else {
                        messages::profile_switching()
                    }
                    .render(locale),
                    if index < 3 {
                        messages::common_enabled()
                    } else {
                        messages::common_disabled()
                    }
                    .render(locale)
                )
                .into(),
                enabled: index < 3,
                docked,
                ..profile_defaults.clone()
            }
        })
        .collect(),
    ));
    ui.set_profile_total(4);
    ui.set_profile_testable(true);
    ui.set_selected_profile(0);
    ui.set_form_id("wezterm".into());
    ui.set_form_name("WezTerm".into());
    ui.set_form_matcher("wezterm-gui.exe".into());
    ui.set_form_hotkey("F3".into());
    ui.set_form_enabled(true);
    ui.set_candidate_labels(model(vec!["wezterm-gui.exe".into()]));
    ui.set_launch_executable(r"C:\Program Files\WezTerm\wezterm-gui.exe".into());
    ui.set_launch_args("[]".into());
    ui.set_profile_help(messages::match_windows().render(locale).into());
    ui.set_diagnostic_loaded(true);
    ui.set_diagnostic_window_control(true);
    ui.set_diagnostic_hotkeys(true);
    ui.set_diagnostic_tray(true);
    ui.set_diagnostic_platform("Windows".into());
    ui.set_diagnostic_backend("Example capabilities".into());
    ui.set_diagnostic_last_action("Example: WezTerm activated successfully".into());
}

const SKILL_APPS: [(&str, &str); 5] = [
    ("Claude Code", r"C:\Users\Demo\.claude\skills"),
    ("Codex", r"C:\Users\Demo\.codex\skills"),
    ("Gemini CLI", r"C:\Users\Demo\.gemini\skills"),
    ("OpenCode", r"C:\Users\Demo\.config\opencode\skills"),
    ("Grok Build", r"C:\Users\Demo\.grok\skills"),
];

fn configure_skills(ui: &AppWindow, locale: Locale, view: i32, scene: &str) {
    let skills = ui.global::<SkillsState>();
    let selected_batch = scene == "skills-selected";
    let selected_problem = scene == "skills-problem";
    let selected_link = scene == "skills-linked";
    skills.set_loaded(true);
    skills.set_view(view);
    skills.set_daily_update(true);
    skills.set_deploy_mode(match SkillDeployMode::default() {
        SkillDeployMode::Symlink => 0,
        SkillDeployMode::Copy => 1,
    });
    skills.set_installed_count(5);
    skills.set_storage_path(r"C:\Users\Demo\AppData\Local\SeanToolbox\skills".into());
    skills.set_schedule_text("Example: last success 1 hour ago · next run in 23 hours".into());
    skills.set_activity_text("Example data · no repository or local directory was accessed".into());
    skills.set_problem_count(1);
    skills.set_affected_skill_count(1);
    skills.set_result_title(messages::skills_attention().render(locale).into());
    skills.set_result_summary(messages::skills_overview(5, 1).render(locale).into());
    skills.set_activity_detail("Example data · no repository or local directory was accessed.\n\ncode-review\nC:\\Users\\Demo\\.codex\\skills\\code-review\nExample: the destination contains a folder not managed by Toolbox. No files were overwritten.".into());
    skills.set_activity_visible(scene == "skills-activity");
    skills.set_details_visible(matches!(
        scene,
        "skills-details" | "skills-problem" | "skills-linked" | "skills-import"
    ));
    if selected_batch {
        skills.set_query("project".into());
        skills.set_filtered(true);
        skills.set_selected_count(3);
        skills.set_visible_selected_count(2);
        skills.set_hidden_selected_count(1);
        skills.set_all_selected(true);
        skills.set_selection_text(messages::skills_selection_scope(1, 3).render(locale).into());
        // Mixed deployments: Claude Code on all three, Codex on two of them.
        skills.set_batch_app0(true);
        skills.set_batch_partial(model(vec![false, true, false, false, false]));
    }
    skills.set_source_options(model(vec![
        messages::common_all().render(locale).into(),
        messages::common_local().render(locale).into(),
        messages::skills_source_linked().render(locale).into(),
        "sean2077/skills".into(),
    ]));
    skills.set_rows(model(
        [
            (
                "agent-scaffold",
                "Project instructions, conventions and worktree tools.",
                false,
                false,
            ),
            (
                "project-docs-organizer",
                "Keep project documentation concise and easy to find.",
                false,
                false,
            ),
            (
                "code-review",
                "Review changes and report actionable findings.",
                true,
                false,
            ),
            (
                "workspace-notes",
                "Team notes maintained in a local knowledge workspace.",
                false,
                true,
            ),
            (
                "release-checklist",
                "Verify release assets, version and published checksums.",
                false,
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        .filter(|(index, _)| !selected_batch || *index < 2)
        .map(|(index, (name, description, problem, linked))| {
            let enabled = index < 4;
            let deployed = [
                enabled && view == 0,
                index < 3 && view == 0,
                index == 1 && view == 0,
                false,
                false,
            ];
            let [app0_hint, app1_hint, app2_hint, app3_hint, app4_hint]: [slint::SharedString; 5] =
                std::array::from_fn(|app| {
                    let (label, path) = SKILL_APPS[app];
                    let destination = format!(r"{path}\{name}");
                    if deployed[app] {
                        messages::skills_app_deployed(label, destination)
                    } else {
                        messages::skills_app_not_deployed(label, destination)
                    }
                    .render(locale)
                    .into()
                });
            SkillListRow {
                id: name.into(),
                name: name.into(),
                description: description.into(),
                detail: if view == 2 {
                    format!(r"C:\Users\Demo\.claude\skills\{name}").into()
                } else {
                    "sean2077/skills · example source".into()
                },
                installed: view == 0,
                problem: problem && view == 0,
                linked,
                selected: selected_batch,
                updatable: view == 0 && !linked,
                source: if linked {
                    messages::skills_source_linked().render(locale).into()
                } else {
                    "sean2077/skills · 7e42a19".into()
                },
                source_target: if linked {
                    r"C:\Work\knowledge\workspace-notes".into()
                } else if view == 2 {
                    format!(r"C:\Users\Demo\.claude\skills\{name}").into()
                } else {
                    "https://github.com/sean2077/skills".into()
                },
                status: if enabled {
                    messages::common_enabled()
                } else {
                    messages::common_disabled()
                }
                .render(locale)
                .into(),
                enabled,
                app0: deployed[0],
                app1: deployed[1],
                app2: deployed[2],
                app0_hint,
                app1_hint,
                app2_hint,
                app3_hint,
                app4_hint,
                deployment_count: deployed.into_iter().filter(|enabled| *enabled).count() as i32,
                ..Default::default()
            }
        })
        .collect(),
    ));
    let (selected_index, selected_name, description) = if selected_problem {
        (
            2,
            "code-review",
            "Review changes and report actionable findings.",
        )
    } else if selected_link {
        (
            3,
            "workspace-notes",
            "Team notes maintained in a local knowledge workspace.",
        )
    } else {
        (
            0,
            "agent-scaffold",
            "Project instructions, conventions and worktree tools.",
        )
    };
    let selected_path = if selected_link {
        r"C:\Work\knowledge\workspace-notes".to_owned()
    } else if view == 2 {
        format!(r"C:\Users\Demo\.claude\skills\{selected_name}")
    } else {
        format!(r"C:\Users\Demo\AppData\Local\SeanToolbox\skills\{selected_name}")
    };
    skills.set_selected_id(selected_name.into());
    skills.set_selected_managed_id(if view == 0 {
        selected_name.into()
    } else {
        "".into()
    });
    skills.set_selected_index(selected_index);
    skills.set_selected_name(selected_name.into());
    skills.set_selected_description(description.into());
    skills.set_selected_source(if selected_link {
        messages::skills_source_linked().render(locale).into()
    } else if view == 2 {
        selected_path.clone().into()
    } else {
        "sean2077/skills · 7e42a19".into()
    });
    let revision = if selected_link { "—" } else { "7e42a19" };
    skills.set_selected_detail(
        messages::skills_detail(selected_name, revision, "1h")
            .render(locale)
            .into(),
    );
    skills.set_selected_summary(if view == 0 {
        messages::skills_selected_summary(selected_name, revision, "1h")
            .render(locale)
            .into()
    } else {
        "Example data · review before importing".into()
    });
    skills.set_selected_master_path(selected_path.clone().into());
    skills.set_selected_source_target(if selected_link || view == 2 {
        selected_path.clone().into()
    } else {
        "https://github.com/sean2077/skills".into()
    });
    skills.set_selected_linked(selected_link);
    if selected_problem {
        skills.set_selected_error("Example: the destination contains a folder not managed by Toolbox. No files were overwritten.".into());
    }
    skills.set_selected_installed(view == 0);
    skills.set_selected_updatable(view == 0 && !selected_link);
    if scene == "skills-import" {
        skills.set_import_from_folder(true);
        skills.set_import_path(selected_path.into());
        skills.set_link_import(false);
    }
    skills.set_apps(model(
        SKILL_APPS
            .into_iter()
            .enumerate()
            .map(|(index, (name, path))| SkillAppRow {
                index: index as i32,
                name: name.into(),
                path: if view == 0 {
                    format!(r"{path}\{selected_name}").into()
                } else {
                    path.into()
                },
                enabled: index < if selected_link { 1 } else { 2 },
            })
            .collect(),
    ));
    skills.set_repositories(model(vec![
        SkillRepositoryRow {
            id: "personal".into(),
            url: "https://github.com/sean2077/skills".into(),
            detail: "main · 5 example skills".into(),
            removable: true,
            ..Default::default()
        },
        SkillRepositoryRow {
            id: "official".into(),
            url: "https://github.com/anthropics/skills".into(),
            detail: "main · example source".into(),
            removable: true,
            ..Default::default()
        },
    ]));
}

fn render_frame(adapter: &CaptureWindow, pixels: &mut SharedPixelBuffer<Rgb8Pixel>) {
    slint::platform::update_timers_and_animations();
    adapter
        .renderer
        .render(pixels.make_mut_slice(), adapter.size().width as usize);
}

fn probe_controls(
    ui: &AppWindow,
    adapter: &CaptureWindow,
    pixels: &mut SharedPixelBuffer<Rgb8Pixel>,
    scene: &str,
    points: &BTreeMap<String, [f32; 2]>,
) -> Result<Vec<&'static str>, Box<dyn Error>> {
    let point = |name: &str| -> Result<slint::LogicalPosition, Box<dyn Error>> {
        let [x, y] = *points
            .get(name)
            .ok_or_else(|| format!("missing smoke point: {name}"))?;
        Ok(slint::LogicalPosition::new(x, y))
    };
    let click = |name| -> Result<(), Box<dyn Error>> {
        let position = point(name)?;
        adapter
            .window
            .dispatch_event(WindowEvent::PointerMoved { position });
        adapter.window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
        adapter.window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
        Ok(())
    };
    let mut verified = Vec::new();
    match scene {
        "settings" => {
            let original = ui.get_setting_theme();
            for (name, value) in [("theme-dark", 2), ("theme-light", 1), ("theme-system", 0)] {
                click(name)?;
                if ui.get_setting_theme() != value {
                    return Err(format!("smoke point {name} did not select its theme").into());
                }
                verified.push(name);
                render_frame(adapter, pixels);
            }
            let reset = Rc::new(Cell::new(false));
            let observed = reset.clone();
            ui.on_reset_settings(move || observed.set(true));
            ui.set_settings_dirty(true);
            render_frame(adapter, pixels);
            click("settings-discard")?;
            if !reset.get() {
                return Err("smoke point settings-discard did not request discard".into());
            }
            verified.push("settings-discard");
            ui.set_setting_theme(original);
            ui.set_settings_dirty(false);
        }
        "apps" => {
            let selected = Rc::new(Cell::new(-1));
            let observed = selected.clone();
            ui.on_edit_profile(move |index| observed.set(index));
            click("profile-first")?;
            if selected.get() != 0 {
                return Err("smoke point profile-first did not select source index 0".into());
            }
            verified.push("profile-first");
            let original_shortcut = ui.get_form_hotkey();
            click("profile-shortcut")?;
            let control = char::from(slint::platform::Key::Control).to_string();
            for text in [control.as_str(), "a"] {
                adapter
                    .window
                    .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
            }
            for text in ["a", control.as_str()] {
                adapter
                    .window
                    .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
            }
            adapter
                .window
                .dispatch_event(WindowEvent::KeyPressed { text: "F9".into() });
            adapter
                .window
                .dispatch_event(WindowEvent::KeyReleased { text: "F9".into() });
            if ui.get_form_hotkey() != "F9" {
                return Err("smoke point profile-shortcut did not edit the shortcut field".into());
            }
            verified.push("profile-shortcut");
            ui.set_form_hotkey(original_shortcut);
            let recording = Rc::new(Cell::new(0));
            let observed = recording.clone();
            ui.on_start_shortcut(move |field| observed.set(field));
            click("profile-record")?;
            if recording.get() != 1 {
                return Err("smoke point profile-record did not request the app recorder".into());
            }
            verified.push("profile-record");
            // Only the presentation of an acknowledged recorder is simulated.
            // Native smoke verifies the actual host lease and registered F8 route.
            ui.set_shortcut_recording(1);
            ui.invoke_focus_shortcut_recorder();
            render_frame(adapter, pixels);
            let position = point("profile-record-fill")?.to_physical(adapter.window.scale_factor());
            let color = pixels.as_slice()
                [position.y as usize * adapter.size().width as usize + position.x as usize];
            if color.b.saturating_sub(color.r) <= 60 {
                return Err(
                    "smoke point profile-record-fill does not sample an active recorder".into(),
                );
            }
            verified.push("profile-record-fill");
            ui.set_shortcut_recording(0);
            render_frame(adapter, pixels);
            click("quake-toggle")?;
            if !ui.get_form_quake_enabled() {
                return Err("smoke point quake-toggle did not enable docking".into());
            }
            verified.push("quake-toggle");
            render_frame(adapter, pixels);
            let position = point("quake-preview-fill")?.to_physical(adapter.window.scale_factor());
            let color = pixels.as_slice()
                [position.y as usize * adapter.size().width as usize + position.x as usize];
            if color.b.saturating_sub(color.r) <= 10 {
                return Err(
                    "smoke point quake-preview-fill does not sample the enabled preview".into(),
                );
            }
            verified.push("quake-preview-fill");
            click("quake-right-preset")?;
            if ui.get_form_quake_edge() != 3
                || ui.get_form_quake_width() != 40
                || ui.get_form_quake_height() != 100
            {
                return Err(
                    "smoke point quake-right-preset did not choose the right-side preset".into(),
                );
            }
            verified.push("quake-right-preset");
            ui.set_form_quake_enabled(false);
            ui.set_form_quake_edge(0);
            ui.set_form_quake_width(100);
            ui.set_form_quake_height(50);
            ui.invoke_focus_profile_program();
        }
        _ => return Err(format!("control verification is unsupported for {scene}").into()),
    }
    render_frame(adapter, pixels);
    Ok(verified)
}

fn main() -> Result<(), Box<dyn Error>> {
    // The Python entry point validates these values and documents the interface.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !(7..=11).contains(&args.len()) {
        return Err("Use scripts/render_ui.py (output scene locale theme width height scale [font-family] [font-file] [allow-unsupported-size] [control-points-json])".into());
    }
    let output = PathBuf::from(&args[0]);
    let scene = args[1].as_str();
    let locale = Locale::from_tag(&args[2]);
    let theme = match args[3].as_str() {
        "light" => slint::language::ColorScheme::Light,
        "dark" => slint::language::ColorScheme::Dark,
        _ => return Err("theme must be light or dark".into()),
    };
    let width: u32 = args[4].parse()?;
    let height: u32 = args[5].parse()?;
    let scale: f32 = args[6].parse()?;
    if !(240..=4096).contains(&width)
        || !(200..=4096).contains(&height)
        || !(0.5..=3.0).contains(&scale)
    {
        return Err("unsupported capture size or scale".into());
    }
    let platform = CapturePlatform::default();
    let windows = platform.windows.clone();
    slint::platform::set_platform(Box::new(platform))?;
    let manager = AppWindow::new()?;
    slint::select_bundled_translation(locale.tag())?;
    configure_manager(&manager, locale, theme, scene);
    let launcher = if matches!(scene, "launcher" | "launcher-details") {
        Some(LauncherWindow::new()?)
    } else {
        None
    };
    let adapter = windows
        .borrow()
        .last()
        .cloned()
        .ok_or("missing software window")?;
    if let Some(path) = args.get(8).filter(|path| !path.is_empty()) {
        adapter
            .renderer()
            .register_font_from_path(&PathBuf::from(path))?;
    }
    if let Some(font) = args.get(7).filter(|font| !font.is_empty()) {
        manager.set_platform_font_family(font.into());
        if let Some(launcher) = &launcher {
            launcher.set_platform_font_family(font.into());
        }
    }
    if let Some(launcher) = &launcher {
        launcher.global::<Palette>().set_color_scheme(theme);
        launcher.set_loaded(true);
        launcher.set_projects(projects());
        launcher.set_project_total(6);
        launcher.set_selected_project(0);
        launcher.set_panel_width(width as f32);
        launcher.set_panel_height(height as f32);
        launcher.show()?;
        launcher.invoke_focus_search();
        launcher.set_details_expanded(scene == "launcher-details");
    } else {
        match scene {
            "projects" => manager.set_page(0),
            "apps" | "apps-advanced" | "quake" => {
                manager.set_page(1);
                manager.set_editing(true);
                manager.set_form_quake_enabled(scene == "quake");
                manager.set_form_wezterm_dropdown(scene == "quake");
                manager.set_form_quake_topmost(true);
                manager.set_form_quake_auto_hide(true);
                manager.set_form_advanced(scene == "apps-advanced");
            }
            "settings" => manager.set_page(2),
            "status" => manager.set_page(3),
            "skills"
            | "skills-discover"
            | "skills-local"
            | "skills-repositories"
            | "skills-details"
            | "skills-problem"
            | "skills-linked"
            | "skills-import"
            | "skills-selected"
            | "skills-activity" => {
                manager.set_page(4);
                let view = match scene {
                    "skills-discover" => 1,
                    "skills-local" | "skills-import" => 2,
                    "skills-repositories" => 3,
                    _ => 0,
                };
                configure_skills(&manager, locale, view, scene);
            }
            "import" => {
                manager.set_page(2);
                manager.set_import_visible(true);
                manager.set_import_is_json(true);
                manager.set_import_summary(
                    "Example configuration\n\nWezTerm · F3 · top half\nVS Code · F4\nChrome · F5"
                        .into(),
                );
            }
            "confirm" => {
                manager.set_page(1);
                manager.set_confirm_visible(true);
                manager.set_confirm_title("Example: delete this application configuration?".into());
                manager.set_confirm_message(
                    "WezTerm remains installed. Only its toolbox configuration is removed.".into(),
                );
                manager.set_confirm_accept_label(
                    messages::action_delete_profile().render(locale).into(),
                );
                manager.set_confirm_cancel_label(messages::action_cancel().render(locale).into());
                manager.set_confirm_destructive(true);
            }
            _ => return Err(format!("unknown scene: {scene}").into()),
        }
        manager.show()?;
    }
    adapter
        .window
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: scale,
        });
    let physical = PhysicalSize::new(
        (width as f32 * scale).round() as u32,
        (height as f32 * scale).round() as u32,
    );
    adapter.window.set_size(physical);
    let mut pixels = SharedPixelBuffer::<Rgb8Pixel>::new(physical.width, physical.height);
    render_frame(&adapter, &mut pixels);
    if scene == "quake" {
        adapter.window.dispatch_event(WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(width as f32 - 43.0, height as f32 / 2.0),
            delta_x: 0.0,
            delta_y: -260.0,
        });
        render_frame(&adapter, &mut pixels);
    } else if scene == "apps-advanced" {
        manager.invoke_focus_profile_problem(4);
        render_frame(&adapter, &mut pixels);
    }
    let verified = if let Some(points) = args.get(10).filter(|points| !points.is_empty()) {
        probe_controls(
            &manager,
            &adapter,
            &mut pixels,
            scene,
            &serde_json::from_str(points)?,
        )?
    } else {
        Vec::new()
    };
    let actual = adapter.window.size();
    if actual != physical {
        return Err(
            format!("window size {actual:?} does not match requested buffer {physical:?}").into(),
        );
    }
    let constraints = adapter
        .constraints
        .get()
        .ok_or("window did not report layout constraints")?;
    let logical = actual.to_logical(adapter.window.scale_factor());
    let within_bounds = constraints
        .min
        .is_none_or(|min| logical.width >= min.width && logical.height >= min.height)
        && constraints
            .max
            .is_none_or(|max| logical.width <= max.width && logical.height <= max.height);
    if !within_bounds && args.get(9).is_none_or(|allow| allow != "true") {
        return Err(format!("requested {logical:?} is outside Slint layout constraints {constraints:?}; use --allow-unsupported-size only for an explicit comparison").into());
    }
    let mut file = BufWriter::new(File::create(output)?);
    writeln!(file, "P6\n{} {}\n255", physical.width, physical.height)?;
    file.write_all(pixels.as_bytes())?;
    file.flush()?;
    println!(
        "{}",
        serde_json::json!({
            "window_logical_size": [logical.width, logical.height],
            "window_physical_size": [actual.width, actual.height],
            "layout_minimum": constraints.min.map(|size| [size.width, size.height]),
            "layout_maximum": constraints.max.map(|size| [size.width, size.height]),
            "layout_preferred": [constraints.preferred.width, constraints.preferred.height],
            "within_layout_bounds": within_bounds,
            "controls_verified": verified,
        })
    );
    Ok(())
}
