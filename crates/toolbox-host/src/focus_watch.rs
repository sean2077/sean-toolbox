//! Foreground notifications are delivered by Windows to the host's existing
//! message loop. There is no focus polling thread or keyboard hook.

/// Coalesce focus changes while the window worker is busy. At most one check
/// can be queued or running, and a change during that check schedules one more.
#[derive(Default)]
pub(crate) struct FocusQueue {
    dirty: bool,
    pending: bool,
}

impl FocusQueue {
    pub(crate) fn changed(&mut self) {
        self.dirty = true;
    }

    pub(crate) fn dispatch(&mut self, send: impl FnOnce() -> bool) {
        if self.dirty && !self.pending && send() {
            self.dirty = false;
            self.pending = true;
        }
    }

    pub(crate) fn completed(&mut self) {
        self.pending = false;
    }

    pub(crate) fn pending(&self) -> bool {
        self.pending
    }

    pub(crate) fn clear(&mut self) {
        self.dirty = false;
    }
}

#[cfg(windows)]
mod native {
    use crate::runtime::Event;
    use std::{cell::RefCell, marker::PhantomData, rc::Rc};
    use windows_sys::Win32::{
        Foundation::HWND,
        UI::{
            Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent},
            WindowsAndMessaging::{EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT},
        },
    };
    use winit::event_loop::EventLoopProxy;

    struct HookState {
        handle: HWINEVENTHOOK,
        generation: u64,
        queued: bool,
        proxy: EventLoopProxy<Event>,
    }

    thread_local! {
        static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
        static NEXT_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    /// The !Send marker makes installing and unhooking on the same thread an
    /// ownership requirement, as required by UnhookWinEvent.
    pub(crate) struct FocusWatch {
        handle: HWINEVENTHOOK,
        generation: u64,
        _same_thread: PhantomData<Rc<()>>,
    }

    impl FocusWatch {
        pub(crate) fn new(proxy: EventLoopProxy<Event>) -> Result<Self, String> {
            if STATE.with(|state| state.borrow().is_some()) {
                return Err("A foreground watcher is already installed".into());
            }
            // SAFETY: the callback has the WinEventProc ABI and a static
            // lifetime. OUTOFCONTEXT delivers it on this thread's winit message
            // loop without loading code into another application's process.
            let handle = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    std::ptr::null_mut(),
                    Some(foreground_changed),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            if handle.is_null() {
                return Err("无法监听前台窗口变化 / Cannot watch foreground changes: SetWinEventHook returned a null handle".into());
            }
            let generation = NEXT_GENERATION.with(|next| {
                let generation = next.get().wrapping_add(1);
                next.set(generation);
                generation
            });
            STATE.with(|state| {
                *state.borrow_mut() = Some(HookState {
                    handle,
                    generation,
                    queued: false,
                    proxy,
                });
            });
            Ok(Self {
                handle,
                generation,
                _same_thread: PhantomData,
            })
        }

        /// Ignore messages that were queued by an old, already removed hook.
        pub(crate) fn acknowledge(&self, generation: u64) -> bool {
            if generation != self.generation {
                return false;
            }
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                if let Some(state) = state.as_mut()
                    && state.handle == self.handle
                    && state.generation == generation
                {
                    state.queued = false;
                    true
                } else {
                    false
                }
            })
        }
    }

    impl Drop for FocusWatch {
        fn drop(&mut self) {
            // Remove the proxy first, so any reentrant callback becomes a no-op.
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                if state.as_ref().is_some_and(|s| s.handle == self.handle) {
                    *state = None;
                }
            });
            // SAFETY: this non-Send guard is dropped by its installing thread.
            if unsafe { UnhookWinEvent(self.handle) } == 0 {
                eprintln!("Sean Toolbox: UnhookWinEvent failed during foreground watcher cleanup");
            }
        }
    }

    unsafe extern "system" fn foreground_changed(
        hook: HWINEVENTHOOK,
        event: u32,
        _window: HWND,
        _object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if event != EVENT_SYSTEM_FOREGROUND {
            return;
        }
        // Never inspect or manipulate foreign windows from this callback. A
        // queued worker check reads the *current* foreground and window identity
        // again, so delayed notifications cannot hide a newly focused window.
        STATE.with(|state| {
            let Ok(mut state) = state.try_borrow_mut() else {
                return;
            };
            let Some(state) = state.as_mut() else {
                return;
            };
            if state.handle == hook && !state.queued {
                state.queued = state
                    .proxy
                    .send_event(Event::ForegroundChanged(state.generation))
                    .is_ok();
            }
        });
    }
}

#[cfg(windows)]
pub(crate) use native::FocusWatch;

#[cfg(test)]
mod tests {
    use super::FocusQueue;
    use std::cell::Cell;

    #[test]
    fn many_changes_during_a_check_schedule_only_one_followup() {
        let mut queue = FocusQueue::default();
        let sends = Cell::new(0);
        let send = || {
            sends.set(sends.get() + 1);
            true
        };
        queue.changed();
        queue.dispatch(send);
        assert!(queue.pending());
        for _ in 0..100 {
            queue.changed();
            queue.dispatch(send);
        }
        assert_eq!(sends.get(), 1);
        queue.completed();
        queue.dispatch(send);
        assert_eq!(sends.get(), 2);
        queue.completed();
        queue.dispatch(send);
        assert_eq!(sends.get(), 2);
        assert!(!queue.pending());
    }

    #[test]
    fn full_worker_retains_latest_notification_until_capacity_returns() {
        let mut queue = FocusQueue::default();
        queue.changed();
        queue.dispatch(|| false);
        assert!(!queue.pending());
        let sent = Cell::new(false);
        queue.dispatch(|| {
            sent.set(true);
            true
        });
        assert!(sent.get());
        assert!(queue.pending());
    }

    #[test]
    fn disabling_watcher_discards_unsent_focus_changes() {
        let mut queue = FocusQueue::default();
        queue.changed();
        queue.clear();
        queue.dispatch(|| panic!("disabled watcher must not enqueue a check"));
        assert!(!queue.pending());
    }
}
