use std::collections::VecDeque;
use std::time::Duration;
use toolbox_core::{AppProfile, LaunchCommand, Platform};
use toolbox_windows::{Desktop, Engine, EngineOptions, Window};

#[derive(Default)]
struct Fake {
    windows: Vec<Window>,
    focus: Option<String>,
    events: Vec<String>,
    launches: usize,
    start_windows: Vec<Window>,
    launch_results: VecDeque<Result<(), String>>,
    focus_after_launch: Option<Option<String>>,
    focus_after_maximize: Option<Option<String>>,
    unreadable: bool,
    deny_focus: bool,
    reorder_on_focus: bool,
}

impl Desktop for Fake {
    fn check_available(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn windows(&mut self, _: &str) -> Result<Vec<Window>, String> {
        if self.unreadable {
            Err("Permission denied".into())
        } else {
            Ok(self.windows.clone())
        }
    }
    fn foreground(&mut self) -> Result<Option<String>, String> {
        Ok(self.focus.clone())
    }
    fn minimize(&mut self, id: &str) -> Result<(), String> {
        self.events.push(format!("minimize:{id}"));
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or("stale")?
            .minimized = true;
        if self.focus.as_deref() == Some(id) {
            self.focus = None;
        }
        Ok(())
    }
    fn restore(&mut self, id: &str) -> Result<(), String> {
        self.events.push(format!("restore:{id}"));
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or("stale")?
            .minimized = false;
        Ok(())
    }
    fn maximize(&mut self, id: &str) -> Result<(), String> {
        self.events.push(format!("maximize:{id}"));
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or("stale")?
            .maximized = true;
        if let Some(focus) = self.focus_after_maximize.take() {
            self.focus = focus;
        }
        Ok(())
    }
    fn activate(&mut self, id: &str) -> Result<bool, String> {
        self.events.push(format!("activate:{id}"));
        if self.deny_focus {
            return Ok(false);
        }
        if !self.windows.iter().any(|w| w.id == id) {
            return Err("stale".into());
        }
        self.focus = Some(id.to_owned());
        if self.reorder_on_focus {
            self.windows.reverse();
        }
        Ok(true)
    }
    fn launch(&mut self, _: &LaunchCommand) -> Result<(), String> {
        self.launches += 1;
        self.launch_results.pop_front().unwrap_or(Ok(()))?;
        self.windows = self.start_windows.clone();
        if let Some(focus) = self.focus_after_launch.take() {
            self.focus = focus;
        }
        Ok(())
    }
}

fn window(id: &str) -> Window {
    Window {
        id: id.into(),
        title: id.into(),
        minimized: false,
        maximized: false,
    }
}
fn profile() -> AppProfile {
    AppProfile {
        id: "app".into(),
        name: "App".into(),
        platform: Platform::current(),
        matcher: "app.exe".into(),
        hotkey: "F3".into(),
        enabled: true,
        maximize: true,
        cycle_continuous: true,
        launch: vec![
            LaunchCommand {
                executable: "app".into(),
                args: vec![],
                cwd: None,
            },
            LaunchCommand {
                executable: "fallback".into(),
                args: vec![],
                cwd: None,
            },
        ],
    }
}
fn engine(fake: Fake) -> Engine<Fake> {
    Engine::with_options(
        fake,
        EngineOptions {
            launch_timeout: Duration::ZERO,
            poll_interval: Duration::ZERO,
            launch_cooldown: Duration::from_secs(20),
        },
    )
}

#[test]
fn single_foreground_minimizes_then_restores_without_maximizing_existing_window() {
    let mut e = engine(Fake {
        windows: vec![window("a")],
        focus: Some("a".into()),
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    assert!(e.backend_mut().windows[0].minimized);
    e.toggle(&profile()).unwrap();
    assert_eq!(
        e.backend_mut().events,
        ["minimize:a", "restore:a", "activate:a"]
    );
    assert!(!e.backend_mut().windows[0].maximized);
}

#[test]
fn stable_cycle_survives_native_z_order_reordering() {
    let mut e = engine(Fake {
        windows: vec![window("a"), window("b"), window("c")],
        reorder_on_focus: true,
        ..Default::default()
    });
    for _ in 0..4 {
        e.toggle(&profile()).unwrap();
    }
    assert_eq!(
        e.backend_mut().events,
        ["activate:a", "activate:b", "activate:c", "activate:a"]
    );
}

#[test]
fn leaving_application_continues_from_previous_selection() {
    let mut e = engine(Fake {
        windows: vec![window("a"), window("b"), window("c")],
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    e.backend_mut().focus = Some("other-app".into());
    e.toggle(&profile()).unwrap();
    assert_eq!(e.backend_mut().focus.as_deref(), Some("b"));
}

#[test]
fn closing_selected_window_keeps_its_surviving_successor() {
    let mut e = engine(Fake {
        windows: vec![window("a"), window("b"), window("c"), window("d")],
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    e.toggle(&profile()).unwrap();
    e.backend_mut().windows.retain(|w| w.id != "b");
    e.backend_mut().focus = Some("other".into());
    e.toggle(&profile()).unwrap();
    assert_eq!(e.backend_mut().focus.as_deref(), Some("c"));
}

#[test]
fn noncontinuous_cycle_minimizes_all_at_end_and_restarts_at_first() {
    let mut p = profile();
    p.cycle_continuous = false;
    let mut e = engine(Fake {
        windows: vec![window("a"), window("b")],
        ..Default::default()
    });
    e.toggle(&p).unwrap();
    e.toggle(&p).unwrap();
    e.toggle(&p).unwrap();
    assert!(e.backend_mut().windows.iter().all(|w| w.minimized));
    e.toggle(&p).unwrap();
    assert_eq!(e.backend_mut().focus.as_deref(), Some("a"));
    assert_eq!(
        e.backend_mut().events,
        [
            "activate:a",
            "activate:b",
            "minimize:a",
            "minimize:b",
            "restore:a",
            "activate:a"
        ]
    );
}

#[test]
fn launch_only_maximizes_the_new_window_and_stops_after_successful_candidate() {
    let mut e = engine(Fake {
        start_windows: vec![window("new")],
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    assert_eq!(e.backend_mut().events, ["maximize:new", "activate:new"]);
    assert_eq!(e.backend_mut().launches, 1);
}

#[test]
fn failed_spawn_tries_fallback_but_successful_spawn_timeout_never_does() {
    let mut e = engine(Fake {
        launch_results: VecDeque::from([Err("not found".into()), Ok(())]),
        start_windows: vec![window("new")],
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    assert_eq!(e.backend_mut().launches, 2);
    let mut timed_out = engine(Fake::default());
    assert!(
        timed_out
            .toggle(&profile())
            .unwrap_err()
            .contains("timed out")
    );
    assert_eq!(timed_out.backend_mut().launches, 1);
    timed_out.reset();
    assert!(
        timed_out
            .toggle(&profile())
            .unwrap_err()
            .contains("pending")
    );
    assert_eq!(
        timed_out.backend_mut().launches,
        1,
        "reset must not forget a pending spawn"
    );
}

#[test]
fn user_switching_away_during_launch_prevents_both_maximize_and_activation() {
    let mut e = engine(Fake {
        focus: Some("old".into()),
        start_windows: vec![window("new")],
        focus_after_launch: Some(Some("other".into())),
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    assert!(e.backend_mut().events.is_empty());
    assert_eq!(e.backend_mut().focus.as_deref(), Some("other"));
}

#[test]
fn lost_known_foreground_is_not_permission_to_steal_focus() {
    let mut e = engine(Fake {
        focus: Some("old".into()),
        start_windows: vec![window("new")],
        focus_after_launch: Some(None),
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    assert!(e.backend_mut().events.is_empty());
}

#[test]
fn focus_is_rechecked_after_native_maximize_wait() {
    let mut e = engine(Fake {
        focus: Some("old".into()),
        start_windows: vec![window("new")],
        focus_after_maximize: Some(Some("other".into())),
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    assert_eq!(e.backend_mut().events, ["maximize:new"]);
}

#[test]
fn unreadable_window_inventory_is_an_error_and_does_not_launch() {
    let mut e = engine(Fake {
        unreadable: true,
        ..Default::default()
    });
    assert!(e.toggle(&profile()).is_err());
    assert_eq!(e.backend_mut().launches, 0);
}

#[test]
fn two_shortcuts_for_the_same_application_share_the_launch_cooldown() {
    let mut e = engine(Fake::default());
    assert!(e.toggle(&profile()).is_err());
    let mut other = profile();
    other.id = "another-shortcut".into();
    other.matcher = "APP.EXE".into();
    assert!(e.toggle(&other).unwrap_err().contains("pending"));
    assert_eq!(e.backend_mut().launches, 1);
}

#[test]
fn denied_focus_does_not_advance_the_cycle() {
    let mut e = engine(Fake {
        windows: vec![window("a"), window("b")],
        deny_focus: true,
        ..Default::default()
    });
    assert!(e.toggle(&profile()).is_err());
    e.backend_mut().deny_focus = false;
    e.toggle(&profile()).unwrap();
    assert_eq!(e.backend_mut().focus.as_deref(), Some("a"));
}
