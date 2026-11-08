//! A short recorder lease intercepts only this host's registered shortcuts.
//!
//! Registrations remain intact, so neither a failed UI nor an expired recorder
//! needs an OS re-registration. Expiry is checked when an event arrives; the
//! otherwise idle host does not add a timer or polling thread.

use std::time::{Duration, Instant};
use toolbox_core::{ShortcutCaptureAction, ShortcutCaptureState};

const LEASE_DURATION: Duration = Duration::from_secs(3);

struct Lease {
    token: u64,
    expires: Instant,
    shortcut: Option<String>,
}

#[derive(Default)]
pub struct ShortcutCapture {
    lease: Option<Lease>,
}

impl ShortcutCapture {
    fn expire(&mut self, now: Instant) {
        if self
            .lease
            .as_ref()
            .is_some_and(|lease| now >= lease.expires)
        {
            self.lease = None;
        }
    }

    pub fn request(
        &mut self,
        token: u64,
        action: ShortcutCaptureAction,
        now: Instant,
    ) -> ShortcutCaptureState {
        self.expire(now);
        if token == 0 {
            return ShortcutCaptureState::default();
        }
        match action {
            ShortcutCaptureAction::Begin => {
                self.lease = Some(Lease {
                    token,
                    expires: now + LEASE_DURATION,
                    shortcut: None,
                });
            }
            ShortcutCaptureAction::End => {
                if self
                    .lease
                    .as_ref()
                    .is_some_and(|lease| lease.token == token)
                {
                    self.lease = None;
                }
                return ShortcutCaptureState::default();
            }
            ShortcutCaptureAction::Poll => {}
        }
        let Some(lease) = self.lease.as_mut().filter(|lease| lease.token == token) else {
            // A delayed poll must not resurrect an expired or replaced lease.
            return ShortcutCaptureState::default();
        };
        lease.expires = now + LEASE_DURATION;
        ShortcutCaptureState {
            active: true,
            shortcut: lease.shortcut.take(),
        }
    }

    /// True means that this key was consumed and must not launch an app.
    pub fn intercept(&mut self, shortcut: Option<&str>, now: Instant) -> bool {
        self.expire(now);
        let (Some(lease), Some(shortcut)) = (self.lease.as_mut(), shortcut) else {
            return false;
        };
        // Keep the first gesture until the UI acknowledges it. Later keydown
        // events in the same lease remain suppressed rather than executing.
        if lease.shortcut.is_none() {
            lease.shortcut = Some(shortcut.to_owned());
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ShortcutCaptureAction::{Begin, End, Poll};

    #[test]
    fn registered_key_is_reported_once_and_never_dispatched_during_capture() {
        let now = Instant::now();
        let mut capture = ShortcutCapture::default();
        assert!(!capture.intercept(Some("F3"), now));
        assert!(capture.request(17, Begin, now).active);
        assert!(capture.intercept(Some("F3"), now));
        assert!(capture.intercept(Some("F4"), now));
        assert_eq!(
            capture.request(17, Poll, now).shortcut.as_deref(),
            Some("F3")
        );
        assert_eq!(capture.request(17, Poll, now).shortcut, None);
        capture.request(17, End, now);
        assert!(!capture.intercept(Some("F3"), now));
    }

    #[test]
    fn abandoned_recorder_expires_without_a_timer_or_re_registration() {
        let now = Instant::now();
        let mut capture = ShortcutCapture::default();
        capture.request(17, Begin, now);
        assert!(capture.intercept(Some("F3"), now + Duration::from_secs(2)));
        assert!(!capture.intercept(Some("F3"), now + LEASE_DURATION));
        assert!(!capture.request(17, Poll, now + LEASE_DURATION).active);
    }

    #[test]
    fn stale_polls_and_cancellation_do_not_affect_a_new_recorder() {
        let now = Instant::now();
        let mut capture = ShortcutCapture::default();
        capture.request(17, Begin, now);
        capture.request(18, Begin, now);
        assert!(!capture.request(17, End, now).active);
        assert!(capture.intercept(Some("Ctrl+Alt+Space"), now));
        assert_eq!(
            capture.request(18, Poll, now).shortcut.as_deref(),
            Some("Ctrl+Alt+Space")
        );
        assert!(
            !capture
                .request(17, Poll, now + Duration::from_secs(2))
                .active
        );
        assert!(!capture.intercept(Some("F3"), now + LEASE_DURATION));
    }

    #[test]
    fn matching_poll_renews_but_zero_token_cannot_create_or_cancel_a_lease() {
        let now = Instant::now();
        let mut capture = ShortcutCapture::default();
        assert!(!capture.request(0, Begin, now).active);
        assert!(!capture.intercept(Some("F3"), now));
        capture.request(17, Begin, now);
        capture.request(0, End, now);
        assert!(
            capture
                .request(17, Poll, now + Duration::from_secs(2))
                .active
        );
        assert!(capture.intercept(Some("F3"), now + Duration::from_secs(4)));
        assert!(!capture.intercept(None, now + Duration::from_secs(4)));
        assert!(!capture.intercept(Some("F3"), now + Duration::from_secs(5)));
    }
}
