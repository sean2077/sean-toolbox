use crate::backend::{Desktop, Window, WindowRect, quake_bounds};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use toolbox_core::AppProfile;

#[derive(Debug, Clone, Copy)]
pub struct EngineOptions {
    pub launch_timeout: Duration,
    pub poll_interval: Duration,
    pub launch_cooldown: Duration,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            launch_timeout: Duration::from_secs(10),
            poll_interval: Duration::from_millis(50),
            launch_cooldown: Duration::from_secs(20),
        }
    }
}

#[derive(Default)]
struct Cycle {
    order: Vec<String>,
    last_id: Option<String>,
}

#[derive(Clone)]
struct QuakeSession {
    target: String,
    matcher: String,
    hide_on_focus_loss: bool,
    visible: bool,
}

pub struct Engine<D> {
    backend: D,
    cycles: HashMap<String, Cycle>,
    quakes: HashMap<String, QuakeSession>,
    launches: HashMap<String, Instant>,
    options: EngineOptions,
}

impl<D: Desktop> Engine<D> {
    pub fn new(backend: D) -> Self {
        Self::with_options(backend, EngineOptions::default())
    }
    pub fn with_options(backend: D, options: EngineOptions) -> Self {
        Self {
            backend,
            cycles: HashMap::new(),
            quakes: HashMap::new(),
            launches: HashMap::new(),
            options,
        }
    }
    pub fn backend_mut(&mut self) -> &mut D {
        &mut self.backend
    }
    pub fn reset(&mut self) -> Result<(), String> {
        // A hidden terminal must become reachable before forgetting its owner.
        self.backend.clear_quake()?;
        self.cycles.clear();
        self.quakes.clear();
        Ok(())
    }

    /// Called only by a native foreground-change notification, never a timer.
    pub fn foreground_changed(&mut self) -> Result<(), String> {
        let armed = self
            .quakes
            .iter()
            .filter(|(_, session)| session.visible && session.hide_on_focus_loss)
            .map(|(id, session)| (id.clone(), session.clone()))
            .collect::<Vec<_>>();
        if armed.is_empty() || self.backend.foreground()?.is_none() {
            return Ok(());
        }
        let mut errors = Vec::new();
        for (id, session) in armed {
            let result = (|| {
                let windows = self.backend.windows(&session.matcher)?;
                let Some(window) = windows.iter().find(|w| w.id == session.target) else {
                    self.backend.release_quake(&session.target)?;
                    self.quakes.remove(&id);
                    return Ok(());
                };
                if window.minimized || window.hidden {
                    self.backend.release_quake(&session.target)?;
                    if let Some(session) = self.quakes.get_mut(&id) {
                        session.visible = false;
                    }
                    return Ok(());
                }
                // Re-read actual focus after the inventory round trip. Dialogs
                // owned by the target belong to the same interaction session.
                if self.backend.foreground()?.is_some()
                    && !self.backend.is_foreground_family(&session.target)?
                {
                    // A final check cancels hiding if the user refocused. Do
                    // not change stacking until hiding actually succeeds,
                    // or a cancelled hide would remove the visible topmost state.
                    if self.backend.hide_quake_if_unfocused(&session.target)? {
                        if let Some(session) = self.quakes.get_mut(&id) {
                            session.visible = false;
                        }
                        self.backend.release_quake(&session.target)?;
                    }
                }
                Ok::<(), String>(())
            })();
            if let Err(error) = result {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }

    pub fn toggle(&mut self, profile: &AppProfile) -> Result<String, String> {
        if !profile.enabled {
            return Err("Application is disabled / 该应用已禁用".into());
        }
        let prepared = crate::launch::prepare_profile(profile)?;
        let profile = prepared.as_ref();
        if self
            .quakes
            .get(&profile.id)
            .is_some_and(|session| !profile.quake.enabled || session.matcher != profile.matcher)
        {
            self.reset()?;
        }
        self.backend.check_available()?;
        if profile.quake.enabled {
            self.backend.check_quake(&profile.quake)?;
        }
        let foreground = self.backend.foreground()?;
        let windows = self.backend.windows(&profile.matcher)?;
        if let Some(session) = self.quakes.get(&profile.id)
            && !windows.iter().any(|window| window.id == session.target)
        {
            self.backend.release_quake(&session.target)?;
            self.quakes.remove(&profile.id);
        }
        self.launches
            .retain(|_, started| started.elapsed() < self.options.launch_cooldown);
        let launch_key = profile.matcher.to_ascii_lowercase();
        let pending = self.launches.contains_key(&launch_key);
        // WezTerm creates its HWND before initializing the renderer. A hidden
        // window from our pending launch is not a previously hidden session.
        if windows.is_empty() || (pending && windows.iter().all(|window| window.hidden)) {
            if pending {
                return Err(format!(
                    "{}：应用仍在启动，暂不重复执行启动命令 / Launch is still pending; retry after {} seconds",
                    profile.name,
                    self.options.launch_cooldown.as_secs()
                ));
            }
            let placement = if profile.quake.enabled {
                Some(quake_bounds(
                    self.backend.quake_work_area(profile.quake.monitor)?,
                    &profile.quake,
                )?)
            } else {
                None
            };
            return self.start(profile, foreground, placement);
        }
        self.launches.remove(&launch_key);
        if profile.quake.enabled {
            return self.toggle_quake(profile, foreground, windows);
        }
        let available: HashSet<&str> = windows.iter().map(|w| w.id.as_str()).collect();
        let previous = self.cycles.get(&profile.id);
        let mut order: Vec<String> = previous
            .map(|p| {
                p.order
                    .iter()
                    .filter(|id| available.contains(id.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for w in &windows {
            if !order.contains(&w.id) {
                order.push(w.id.clone());
            }
        }

        let target = if windows.len() == 1 {
            let target = &windows[0];
            if foreground.as_ref() == Some(&target.id) {
                self.backend.minimize(&target.id)?;
                self.cycles.insert(
                    profile.id.clone(),
                    Cycle {
                        order,
                        last_id: None,
                    },
                );
                return Ok(format!("{}：已最小化 / Minimized", profile.name));
            }
            target
        } else {
            let mut next = 0;
            if let Some(index) = foreground
                .as_ref()
                .and_then(|id| order.iter().position(|w| w == id))
            {
                next = index + 1;
            } else if let Some(previous) = previous
                && let Some(last_id) = &previous.last_id
            {
                if let Some(index) = order.iter().position(|w| w == last_id) {
                    next = index + 1;
                } else if let Some(old_index) = previous.order.iter().position(|w| w == last_id) {
                    // If the previously selected window closed, retain its
                    // surviving successor instead of skipping one window.
                    next = previous.order[old_index + 1..]
                        .iter()
                        .find_map(|id| order.iter().position(|w| w == id))
                        .unwrap_or(order.len());
                }
            }
            if next >= order.len() {
                if profile.cycle_continuous {
                    next = 0;
                } else {
                    for window in &windows {
                        if !window.minimized {
                            self.backend.minimize(&window.id)?;
                        }
                    }
                    self.cycles.insert(
                        profile.id.clone(),
                        Cycle {
                            order,
                            last_id: None,
                        },
                    );
                    return Ok(format!(
                        "{}：已最小化全部 {} 个窗口 / Minimized all",
                        profile.name,
                        windows.len()
                    ));
                }
            }
            windows
                .iter()
                .find(|w| w.id == order[next])
                .expect("order contains only live windows")
        };
        if target.minimized {
            self.backend.restore(&target.id)?;
        }
        // Restoring may involve a native round trip. If the user changed
        // context during that wait, do not subsequently steal their focus.
        if !self.can_focus(&foreground, &windows)? {
            return Ok(format!(
                "{}：焦点已切换，未抢占 / Focus changed",
                profile.name
            ));
        }
        if !self.backend.activate(&target.id)? {
            return Err(format!(
                "{}：系统拒绝激活窗口，请再次使用快捷键 / Focus request was denied",
                profile.name
            ));
        }
        self.cycles.insert(
            profile.id.clone(),
            Cycle {
                order,
                last_id: Some(target.id.clone()),
            },
        );
        Ok(format!("{}：{}", profile.name, target.title))
    }

    fn can_focus(&mut self, original: &Option<String>, windows: &[Window]) -> Result<bool, String> {
        let now = self.backend.foreground()?;
        Ok(match now {
            Some(id) => original.as_ref() == Some(&id) || windows.iter().any(|w| w.id == id),
            None => original.is_none(),
        })
    }

    fn hide_quake(&mut self, id: &str) -> Result<(), String> {
        self.backend.hide_quake(id)?;
        self.backend.release_quake(id)
    }

    fn remember_quake(&mut self, profile: &AppProfile, target: &str) {
        self.quakes
            .retain(|id, session| id == &profile.id || session.target != target);
        self.quakes.insert(
            profile.id.clone(),
            QuakeSession {
                target: target.into(),
                matcher: profile.matcher.clone(),
                hide_on_focus_loss: profile.quake.hide_on_focus_loss,
                visible: true,
            },
        );
    }

    fn show_quake(
        &mut self,
        profile: &AppProfile,
        target: &Window,
        foreground: &Option<String>,
        windows: &[Window],
        placement: WindowRect,
    ) -> Result<bool, String> {
        if !self.can_focus(foreground, windows)? {
            return Ok(false);
        }
        if self
            .quakes
            .iter()
            .any(|(id, session)| id != &profile.id && session.target == target.id)
        {
            self.backend.release_quake(&target.id)?;
            self.quakes.retain(|_, session| session.target != target.id);
            if !self.can_focus(foreground, windows)? {
                return Ok(false);
            }
        }
        let result = (|| {
            self.backend
                .place_quake(&target.id, placement, profile.quake.always_on_top)?;
            if !self.can_focus(foreground, windows)? {
                return Ok(false);
            }
            if !self.backend.activate(&target.id)? {
                return Err(format!(
                    "{}：系统拒绝激活窗口，请再次使用快捷键 / Focus request was denied",
                    profile.name
                ));
            }
            Ok(true)
        })();
        match result {
            Ok(true) => {
                self.remember_quake(profile, &target.id);
                Ok(true)
            }
            other => {
                // Placement can reveal a hidden window without activating it.
                // If focus moved or activation failed, restore that prior
                // visibility instead of leaving an unarmed dropdown onscreen.
                let hidden = if target.hidden {
                    self.backend.hide_quake(&target.id)
                } else {
                    Ok(())
                };
                let released = self.backend.release_quake(&target.id);
                let cleanup = match (hidden, released) {
                    (Err(a), Err(b)) => Err(format!("{a}\n{b}")),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                    _ => Ok(()),
                };
                match (other, cleanup) {
                    (Err(a), Err(b)) => Err(format!("{a}\n{b}")),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                    _ => Ok(false),
                }
            }
        }
    }

    fn toggle_quake(
        &mut self,
        profile: &AppProfile,
        foreground: Option<String>,
        windows: Vec<Window>,
    ) -> Result<String, String> {
        let previous = self.quakes.get(&profile.id);
        let target = previous
            .and_then(|session| windows.iter().find(|w| w.id == session.target))
            .or_else(|| windows.iter().find(|w| Some(&w.id) == foreground.as_ref()))
            .unwrap_or(&windows[0]);
        // An unclaimed dedicated instance may have appeared after a cold-start
        // timeout or survived a host restart. First attach and position it;
        // only subsequent toggles should dismiss it.
        if foreground.as_ref() == Some(&target.id)
            && (previous.is_some() || !profile.is_windows_wezterm())
        {
            self.hide_quake(&target.id)?;
            self.remember_quake(profile, &target.id);
            self.quakes
                .get_mut(&profile.id)
                .expect("remembered above")
                .visible = false;
            return Ok(format!("{}：已收起 / Hidden", profile.name));
        }
        let placement = quake_bounds(
            self.backend.quake_work_area(profile.quake.monitor)?,
            &profile.quake,
        )?;
        let shown = self.show_quake(profile, target, &foreground, &windows, placement)?;
        Ok(if shown {
            format!("{}：{} / Quake", profile.name, target.title)
        } else {
            format!("{}：焦点已切换，未抢占 / Focus changed", profile.name)
        })
    }

    fn start(
        &mut self,
        profile: &AppProfile,
        foreground: Option<String>,
        placement: Option<WindowRect>,
    ) -> Result<String, String> {
        let mut errors = Vec::new();
        let mut launched = false;
        for command in &profile.launch {
            match self.backend.launch(command) {
                Ok(()) => {
                    launched = true;
                    break;
                }
                Err(error) => errors.push(error),
            }
        }
        if !launched {
            return Err(format!(
                "{}：启动失败 / Launch failed\n{}",
                profile.name,
                errors.join("\n")
            ));
        }
        let start = Instant::now();
        self.launches
            .insert(profile.matcher.to_ascii_lowercase(), start);
        loop {
            let windows = self.backend.windows(&profile.matcher)?;
            if let Some(target) = windows.iter().find(|window| !window.hidden) {
                let mut activated = false;
                if let Some(placement) = placement {
                    activated =
                        self.show_quake(profile, target, &foreground, &windows, placement)?;
                } else if self.can_focus(&foreground, &windows)? {
                    // Maximizing itself can activate a native window. The
                    // guard must precede maximize as well as explicit focus.
                    if profile.maximize && !target.maximized {
                        self.backend.maximize(&target.id)?;
                    }
                    if self.can_focus(&foreground, &windows)? {
                        if target.minimized {
                            self.backend.restore(&target.id)?;
                        }
                        if self.can_focus(&foreground, &windows)? {
                            activated = self.backend.activate(&target.id)?;
                        }
                    }
                }
                self.cycles.insert(
                    profile.id.clone(),
                    Cycle {
                        order: windows.iter().map(|w| w.id.clone()).collect(),
                        last_id: Some(target.id.clone()),
                    },
                );
                self.launches.remove(&profile.matcher.to_ascii_lowercase());
                return Ok(format!(
                    "{}：已启动{}",
                    profile.name,
                    if activated {
                        " / Started"
                    } else {
                        "，保留当前焦点 / Started; kept current focus"
                    }
                ));
            }
            if start.elapsed() >= self.options.launch_timeout {
                break;
            }
            std::thread::sleep(
                self.options
                    .poll_interval
                    .min(self.options.launch_timeout.saturating_sub(start.elapsed())),
            );
        }
        Err(format!(
            "{}：启动命令已执行，但未在 {} 秒内找到窗口；请检查应用标识。为避免重复启动，暂不重试 / Launch timed out",
            profile.name,
            self.options.launch_timeout.as_secs()
        ))
    }
}
