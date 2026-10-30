use std::collections::VecDeque;
use std::time::Duration;
use toolbox_core::{AppProfile, LaunchCommand, Platform, QuakeConfig, QuakeMonitor};
use toolbox_windows::{Desktop, Engine, EngineOptions, Window, WindowRect};

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
    focus_after_place: Option<Option<String>>,
    focus_after_family_check: Option<Option<String>>,
    owned_dialog_for: Option<String>,
    quake_error: Option<String>,
    deny_placement: bool,
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
        self.events.push("launch".into());
        self.launches += 1;
        self.launch_results.pop_front().unwrap_or(Ok(()))?;
        self.windows = self.start_windows.clone();
        if let Some(focus) = self.focus_after_launch.take() {
            self.focus = focus;
        }
        Ok(())
    }
    fn check_quake(&mut self, _: &QuakeConfig) -> Result<(), String> {
        self.quake_error.clone().map_or(Ok(()), Err)
    }
    fn quake_work_area(&mut self, monitor: QuakeMonitor) -> Result<WindowRect, String> {
        self.events.push(format!("monitor:{monitor:?}"));
        Ok(WindowRect {
            x: -1600,
            y: 32,
            width: 1600,
            height: 960,
        })
    }
    fn place_quake(&mut self, id: &str, bounds: WindowRect, above: bool) -> Result<(), String> {
        self.events.push(format!(
            "place:{id}:{}:{}:{}:{}:{above}",
            bounds.x, bounds.y, bounds.width, bounds.height
        ));
        if self.deny_placement {
            return Err("placement denied".into());
        }
        let target = self
            .windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or("stale")?;
        target.minimized = false;
        target.maximized = false;
        if let Some(focus) = self.focus_after_place.take() {
            self.focus = focus;
        }
        Ok(())
    }
    fn release_quake(&mut self, id: &str) -> Result<(), String> {
        self.events.push(format!("release:{id}"));
        Ok(())
    }
    fn clear_quake(&mut self) -> Result<(), String> {
        self.events.push("clear-quake".into());
        Ok(())
    }
    fn is_foreground_family(&mut self, id: &str) -> Result<bool, String> {
        let matches = self.focus.as_deref() == Some(id)
            || (self.focus.as_deref() == Some("dialog")
                && self.owned_dialog_for.as_deref() == Some(id));
        if let Some(focus) = self.focus_after_family_check.take() {
            self.focus = focus;
        }
        Ok(matches)
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
        quake: Default::default(),
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
    assert_eq!(
        e.backend_mut().events,
        ["launch", "maximize:new", "activate:new"]
    );
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
    timed_out.reset().unwrap();
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
    assert_eq!(e.backend_mut().events, ["launch"]);
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
    assert_eq!(e.backend_mut().events, ["launch"]);
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
    assert_eq!(e.backend_mut().events, ["launch", "maximize:new"]);
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

fn quake_profile() -> AppProfile {
    let mut profile = profile();
    profile.quake.enabled = true;
    profile.quake.width_percent = 80;
    profile.quake.always_on_top = true;
    profile
}

#[test]
fn quake_pins_one_window_across_focus_changes_and_preserves_other_instances() {
    let mut e = engine(Fake {
        windows: vec![window("a"), window("b")],
        ..Default::default()
    });
    e.toggle(&quake_profile()).unwrap();
    e.backend_mut().focus = Some("b".into());
    e.toggle(&quake_profile()).unwrap();
    assert_eq!(e.backend_mut().focus.as_deref(), Some("a"));
    e.toggle(&quake_profile()).unwrap();
    assert!(e.backend_mut().windows[0].minimized);
    assert!(!e.backend_mut().windows[1].minimized);
    e.toggle(&quake_profile()).unwrap();
    assert_eq!(e.backend_mut().focus.as_deref(), Some("a"));
    assert_eq!(
        e.backend_mut().events,
        [
            "monitor:Cursor",
            "place:a:-1440:32:1280:480:true",
            "activate:a",
            "monitor:Cursor",
            "place:a:-1440:32:1280:480:true",
            "activate:a",
            "release:a",
            "minimize:a",
            "monitor:Cursor",
            "place:a:-1440:32:1280:480:true",
            "activate:a",
        ]
    );
}

#[test]
fn quake_initially_attaches_the_active_instance_and_recovers_if_it_closes() {
    let mut e = engine(Fake {
        windows: vec![window("a"), window("b")],
        focus: Some("b".into()),
        ..Default::default()
    });
    e.toggle(&quake_profile()).unwrap();
    assert!(!e.backend_mut().windows[0].minimized);
    assert!(e.backend_mut().windows[1].minimized);
    e.backend_mut().windows.retain(|w| w.id != "b");
    e.toggle(&quake_profile()).unwrap();
    assert_eq!(e.backend_mut().focus.as_deref(), Some("a"));
    assert!(
        e.backend_mut()
            .events
            .windows(2)
            .any(|pair| pair == ["release:b", "monitor:Cursor"]),
        "closed targets must release their retained style snapshot before a replacement is attached"
    );
}

#[test]
fn quake_captures_monitor_before_start_and_ignores_legacy_maximize() {
    let mut e = engine(Fake {
        start_windows: vec![window("new")],
        ..Default::default()
    });
    let mut profile = quake_profile();
    profile.quake.monitor = QuakeMonitor::Foreground;
    e.toggle(&profile).unwrap();
    assert_eq!(
        e.backend_mut().events,
        [
            "monitor:Foreground",
            "launch",
            "place:new:-1440:32:1280:480:true",
            "activate:new",
        ]
    );
    assert_eq!(e.backend_mut().launches, 1);
    assert!(!e.backend_mut().windows[0].maximized);
}

#[test]
fn quake_cannot_steal_focus_after_restore_and_releases_temporary_stacking() {
    let mut e = engine(Fake {
        windows: vec![window("a")],
        focus: Some("original".into()),
        focus_after_place: Some(Some("other".into())),
        ..Default::default()
    });
    let mut profile = quake_profile();
    profile.quake.hide_on_focus_loss = true;
    assert!(e.toggle(&profile).unwrap().contains("Focus changed"));
    e.foreground_changed().unwrap();
    assert_eq!(
        e.backend_mut().events,
        [
            "monitor:Cursor",
            "place:a:-1440:32:1280:480:true",
            "release:a"
        ]
    );
    assert_eq!(e.backend_mut().focus.as_deref(), Some("other"));
    assert!(!e.backend_mut().windows[0].minimized);
}

#[test]
fn quake_placement_and_activation_failures_release_styles_and_never_arm_blur() {
    for deny_placement in [true, false] {
        let mut e = engine(Fake {
            windows: vec![window("a")],
            deny_placement,
            deny_focus: !deny_placement,
            ..Default::default()
        });
        let mut profile = quake_profile();
        profile.quake.hide_on_focus_loss = true;
        assert!(e.toggle(&profile).is_err());
        assert_eq!(
            e.backend_mut().events.last().map(String::as_str),
            Some("release:a")
        );
        e.backend_mut().focus = Some("other".into());
        e.foreground_changed().unwrap();
        assert!(!e.backend_mut().windows[0].minimized);
    }
}

#[test]
fn quake_foreground_events_recheck_focus_and_keep_owned_dialogs_open() {
    let mut e = engine(Fake {
        windows: vec![window("a")],
        ..Default::default()
    });
    let mut profile = quake_profile();
    profile.quake.hide_on_focus_loss = true;
    e.toggle(&profile).unwrap();
    e.foreground_changed().unwrap();
    e.backend_mut().focus = None;
    e.foreground_changed().unwrap();
    e.backend_mut().focus = Some("dialog".into());
    e.backend_mut().owned_dialog_for = Some("a".into());
    e.foreground_changed().unwrap();
    assert!(!e.backend_mut().windows[0].minimized);
    e.backend_mut().focus = Some("other".into());
    e.foreground_changed().unwrap();
    assert!(e.backend_mut().windows[0].minimized);
    assert_eq!(&e.backend_mut().events[3..], ["minimize:a", "release:a"]);
    let count = e.backend_mut().events.len();
    e.foreground_changed().unwrap();
    assert_eq!(
        e.backend_mut().events.len(),
        count,
        "one native change must hide only once"
    );
    e.toggle(&profile).unwrap();
    e.foreground_changed().unwrap();
    assert!(
        !e.backend_mut().windows[0].minimized,
        "a queued notification must read current foreground"
    );
}

#[test]
fn quake_reset_clears_armed_sessions_and_unsupported_mode_does_not_spawn() {
    let mut e = engine(Fake {
        windows: vec![window("a")],
        ..Default::default()
    });
    let mut profile = quake_profile();
    profile.quake.hide_on_focus_loss = true;
    e.toggle(&profile).unwrap();
    e.reset().unwrap();
    e.backend_mut().focus = Some("other".into());
    e.foreground_changed().unwrap();
    assert!(!e.backend_mut().windows[0].minimized);
    assert_eq!(
        e.backend_mut().events.last().map(String::as_str),
        Some("clear-quake")
    );
    let mut unsupported = engine(Fake {
        quake_error: Some("unsupported".into()),
        ..Default::default()
    });
    assert!(
        unsupported
            .toggle(&profile)
            .unwrap_err()
            .contains("unsupported")
    );
    assert_eq!(unsupported.backend_mut().launches, 0);
}

#[test]
fn normal_profile_does_not_arm_focus_loss_actions() {
    let mut e = engine(Fake {
        windows: vec![window("a")],
        ..Default::default()
    });
    e.toggle(&profile()).unwrap();
    e.backend_mut().focus = Some("other".into());
    e.foreground_changed().unwrap();
    assert_eq!(e.backend_mut().events, ["activate:a"]);
    assert!(!e.backend_mut().windows[0].minimized);
}

#[test]
fn quake_refocusing_before_minimizing_cancels_auto_hide_without_changing_stacking() {
    let mut e = engine(Fake {
        windows: vec![window("a")],
        ..Default::default()
    });
    let mut profile = quake_profile();
    profile.quake.hide_on_focus_loss = true;
    e.toggle(&profile).unwrap();
    e.backend_mut().focus = Some("other".into());
    e.backend_mut().focus_after_family_check = Some(Some("a".into()));
    e.foreground_changed().unwrap();
    assert!(!e.backend_mut().windows[0].minimized);
    assert_eq!(e.backend_mut().focus.as_deref(), Some("a"));
    assert_eq!(
        e.backend_mut().events.last().map(String::as_str),
        Some("activate:a")
    );
    assert!(
        !e.backend_mut()
            .events
            .iter()
            .any(|event| event.starts_with("release:") || event.starts_with("minimize:")),
        "a cancelled hide must leave the shown window's stacking intact"
    );
}

#[test]
fn quake_window_has_one_owner_when_profiles_share_a_matcher() {
    let mut e = engine(Fake {
        windows: vec![window("a")],
        ..Default::default()
    });
    let mut first = quake_profile();
    first.quake.hide_on_focus_loss = true;
    e.toggle(&first).unwrap();
    e.backend_mut().focus = Some("other".into());
    let mut second = quake_profile();
    second.id = "other-profile".into();
    second.quake.always_on_top = false;
    e.toggle(&second).unwrap();
    e.backend_mut().focus = Some("other".into());
    e.foreground_changed().unwrap();
    assert!(
        !e.backend_mut().windows[0].minimized,
        "old opt-in must not survive ownership transfer"
    );
    assert!(
        e.backend_mut()
            .events
            .windows(2)
            .any(|pair| pair == ["release:a", "place:a:-1440:32:1280:480:false"])
    );
}
