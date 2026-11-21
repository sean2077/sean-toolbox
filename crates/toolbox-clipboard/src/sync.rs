//! Echo-loop suppression and debouncing.
//!
//! Windows often emits several clipboard updates for one copy. Coalesce them,
//! then suppress content that matches the last sent or received fingerprint.
//! There is deliberately no periodic content send: an idle system stays silent.

use crate::model::ClipboardContent;
use std::time::{Duration, Instant};

pub const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug)]
struct Pending {
    content: ClipboardContent,
    hash: String,
    at: Instant,
}

#[derive(Debug, Default)]
pub struct LoopGuard {
    last_sent: Option<String>,
    last_received: Option<String>,
    pending: Option<Pending>,
}

impl LoopGuard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record content received from the peer and written locally. Its echo must
    /// not be sent back.
    pub fn note_received(&mut self, content: &ClipboardContent) {
        self.last_received = Some(content.fingerprint());
        // A remote write must not suppress the same local content later: the
        // origin marker is the primary echo guard, the hash is only a fallback.
        self.last_sent = None;
    }

    /// Record content successfully sent to the peer.
    pub fn note_sent(&mut self, content: &ClipboardContent) {
        self.last_sent = Some(content.fingerprint());
        self.last_received = None;
    }

    pub fn last_sent(&self) -> Option<&str> {
        self.last_sent.as_deref()
    }

    pub fn last_received(&self) -> Option<&str> {
        self.last_received.as_deref()
    }

    /// Queue a local clipboard observation. Empty content or content that
    /// matches the last sent/received snapshot is dropped immediately.
    pub fn observe(&mut self, content: ClipboardContent, now: Instant) {
        if content.is_empty() {
            self.pending = None;
            return;
        }
        let hash = content.fingerprint();
        if Some(&hash) == self.last_sent.as_ref() || Some(&hash) == self.last_received.as_ref() {
            self.pending = None;
            return;
        }
        self.pending = Some(Pending {
            content,
            hash,
            at: now,
        });
    }

    /// Return content whose debounce window has elapsed. The caller is
    /// responsible for calling `note_sent` only after the transfer succeeds.
    pub fn ready(&mut self, now: Instant) -> Option<(ClipboardContent, String)> {
        let pending = self.pending.as_ref()?;
        if now.duration_since(pending.at) < DEBOUNCE {
            return None;
        }
        let pending = self.pending.take()?;
        if Some(&pending.hash) == self.last_sent.as_ref()
            || Some(&pending.hash) == self.last_received.as_ref()
        {
            return None;
        }
        Some((pending.content, pending.hash))
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.as_ref().map(|pending| pending.at + DEBOUNCE)
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    fn text(value: &str) -> ClipboardContent {
        ClipboardContent {
            text: Some(value.into()),
            ..Default::default()
        }
    }

    #[test]
    fn idle_never_returns_content() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        assert!(guard.ready(now + Duration::from_secs(60)).is_none());
    }

    #[test]
    fn debounce_coalesces_rapid_updates() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        guard.observe(text("a"), now);
        guard.observe(text("b"), now + Duration::from_millis(50));
        assert!(guard.ready(now + Duration::from_millis(100)).is_none());
        let (content, _) = guard
            .ready(now + Duration::from_millis(300))
            .expect("debounced content");
        assert_eq!(content.text.as_deref(), Some("b"));
    }

    #[test]
    fn received_content_is_not_echoed() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        let content = text("from-peer");
        guard.note_received(&content);
        guard.observe(content, now);
        assert!(guard.ready(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn sent_content_is_not_echoed_again() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        let content = text("to-peer");
        guard.note_sent(&content);
        guard.observe(content, now);
        assert!(guard.ready(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn one_copy_produces_one_send() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        guard.observe(text("once"), now);
        let (content, hash) = guard.ready(now + Duration::from_secs(1)).unwrap();
        guard.note_sent(&content);
        assert_eq!(content.text.as_deref(), Some("once"));
        assert!(guard.ready(now + Duration::from_secs(2)).is_none());
        assert_eq!(guard.last_sent(), Some(hash.as_str()));
    }

    #[test]
    fn same_content_after_a_remote_copy_is_sent_again() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        let a = text("A");
        guard.note_sent(&a);
        guard.note_received(&text("B"));
        guard.observe(a, now);
        assert_eq!(
            guard
                .ready(now + Duration::from_secs(1))
                .unwrap()
                .0
                .text
                .as_deref(),
            Some("A")
        );
    }

    #[test]
    fn empty_content_clears_pending() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        guard.observe(text("x"), now);
        guard.observe(ClipboardContent::default(), now);
        assert!(guard.ready(now + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn deadline_tracks_pending_content() {
        let mut guard = LoopGuard::new();
        let now = Instant::now();
        assert!(guard.next_deadline().is_none());
        guard.observe(text("x"), now);
        assert_eq!(guard.next_deadline(), Some(now + DEBOUNCE));
        sleep(Duration::from_millis(1));
    }
}
