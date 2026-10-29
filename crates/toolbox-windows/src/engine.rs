use crate::backend::{Desktop, Window};
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
            launch_timeout: Duration::from_secs(3),
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

pub struct Engine<D> {
    backend: D,
    cycles: HashMap<String, Cycle>,
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
            launches: HashMap::new(),
            options,
        }
    }
    pub fn backend_mut(&mut self) -> &mut D {
        &mut self.backend
    }
    pub fn reset(&mut self) {
        self.cycles.clear();
    }

    pub fn toggle(&mut self, profile: &AppProfile) -> Result<String, String> {
        if !profile.enabled {
            return Err("Application is disabled / 该应用已禁用".into());
        }
        self.backend.check_available()?;
        let foreground = self.backend.foreground()?;
        let windows = self.backend.windows(&profile.matcher)?;
        self.launches
            .retain(|_, started| started.elapsed() < self.options.launch_cooldown);
        let launch_key = profile.matcher.to_ascii_lowercase();
        if windows.is_empty() {
            if self
                .launches
                .get(&launch_key)
                .is_some_and(|t| t.elapsed() < self.options.launch_cooldown)
            {
                return Err(format!(
                    "{}：应用仍在启动，暂不重复执行启动命令 / Launch is still pending; retry after {} seconds",
                    profile.name,
                    self.options.launch_cooldown.as_secs()
                ));
            }
            return self.start(profile, foreground);
        }
        self.launches.remove(&launch_key);
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

    fn start(
        &mut self,
        profile: &AppProfile,
        foreground: Option<String>,
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
            if let Some(target) = windows.first() {
                let mut activated = false;
                if self.can_focus(&foreground, &windows)? {
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
