//! Platform backend boundary and an in-memory backend for tests.

use crate::model::{ClipboardContent, DeferredOffer};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Sender},
};
use std::time::Duration;

pub const ORIGIN_MIME: &str = "application/x-sean-toolbox-origin";
pub const ORIGIN_WINDOWS_FORMAT: &str = "SeanToolbox.ClipOrigin";
/// A text/image/HTML fetch should finish quickly on a LAN.
pub const DEFERRED_TIMEOUT: Duration = Duration::from_secs(15);
/// Files can be large; the paste call blocks while they are staged.
pub const DEFERRED_FILE_TIMEOUT: Duration = Duration::from_secs(60);

pub struct FetchRequest {
    pub id: String,
    pub reply: Sender<Result<ClipboardContent, String>>,
}

/// Backend-to-service request channel used when the operating system asks a
/// deferred clipboard entry to render its payload.
#[derive(Clone, Debug)]
pub struct Fetcher {
    requests: Sender<FetchRequest>,
    connected: Arc<AtomicBool>,
}

impl Fetcher {
    pub fn new(requests: Sender<FetchRequest>, connected: Arc<AtomicBool>) -> Self {
        Self {
            requests,
            connected,
        }
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::Release);
    }

    pub fn fetch(&self, id: &str, timeout: Duration) -> Result<ClipboardContent, String> {
        if !self.connected.load(Ordering::Acquire) {
            return Err("peer is not connected".into());
        }
        let (reply, response) = mpsc::channel();
        self.requests
            .send(FetchRequest {
                id: id.to_owned(),
                reply,
            })
            .map_err(|_| "clipboard service is not running".to_string())?;
        match response.recv_timeout(timeout) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                Err("timed out waiting for the peer to send the clipboard".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("clipboard service stopped".into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadContent {
    pub content: ClipboardContent,
    /// The content carries our origin marker and was written by the peer bridge.
    pub remote_origin: bool,
    /// The source asked clipboard managers not to record this content.
    pub sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendEvent {
    Changed,
    Unavailable,
}

pub trait Backend: Send {
    fn watch(&mut self, events: Sender<BackendEvent>) -> Result<(), String>;
    fn read(&mut self) -> Result<ReadContent, String>;
    fn write(&mut self, content: &ClipboardContent) -> Result<(), String>;

    /// Install the channel used by deferred clipboard entries. Backends that
    /// do not support deferred rendering can ignore it.
    fn set_fetcher(&mut self, _fetcher: Fetcher) {}

    /// Install metadata as a deferred clipboard entry. The backend must not
    /// request the payload until an application actually pastes. Returning an
    /// error tells the service to fall back to an immediate fetch.
    fn defer(&mut self, _offer: &DeferredOffer) -> Result<(), String> {
        Err("deferred clipboard is not supported by this backend".into())
    }
}

#[derive(Debug, Default)]
struct FakeState {
    content: ClipboardContent,
    remote_origin: bool,
    sensitive: bool,
    watchers: Vec<Sender<BackendEvent>>,
    writes: Vec<ClipboardContent>,
    fetcher: Option<Fetcher>,
    deferred: Option<DeferredOffer>,
    deferred_offers: Vec<DeferredOffer>,
}

/// A backend handle for loopback integration tests and unit tests.
#[derive(Clone, Default)]
pub struct FakeBackend {
    state: Arc<Mutex<FakeState>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Simulate a user copying content in a local application.
    pub fn set_local(&self, content: ClipboardContent) {
        let mut state = self.state.lock().unwrap();
        state.content = content;
        state.remote_origin = false;
        state.sensitive = false;
        state.deferred = None;
        for watcher in &state.watchers {
            let _ = watcher.send(BackendEvent::Changed);
        }
    }

    pub fn set_sensitive_local(&self, content: ClipboardContent) {
        let mut state = self.state.lock().unwrap();
        state.content = content;
        state.remote_origin = false;
        state.sensitive = true;
        state.deferred = None;
        for watcher in &state.watchers {
            let _ = watcher.send(BackendEvent::Changed);
        }
    }

    pub fn current(&self) -> ClipboardContent {
        self.state.lock().unwrap().content.clone()
    }

    pub fn writes(&self) -> Vec<ClipboardContent> {
        self.state.lock().unwrap().writes.clone()
    }

    pub fn clear_writes(&self) {
        self.state.lock().unwrap().writes.clear();
    }

    pub fn deferred(&self) -> Option<DeferredOffer> {
        self.state.lock().unwrap().deferred.clone()
    }

    pub fn deferred_offers(&self) -> Vec<DeferredOffer> {
        self.state.lock().unwrap().deferred_offers.clone()
    }

    /// Simulate the operating system rendering a deferred clipboard entry.
    pub fn fetch(&self, id: &str, timeout: Duration) -> Result<ClipboardContent, String> {
        let fetcher = self
            .state
            .lock()
            .unwrap()
            .fetcher
            .clone()
            .ok_or("no fetcher installed")?;
        fetcher.fetch(id, timeout)
    }
}

impl Backend for FakeBackend {
    fn watch(&mut self, events: Sender<BackendEvent>) -> Result<(), String> {
        self.state.lock().unwrap().watchers.push(events);
        Ok(())
    }

    fn read(&mut self) -> Result<ReadContent, String> {
        let state = self.state.lock().unwrap();
        if state.deferred.is_some() {
            return Ok(ReadContent {
                content: ClipboardContent::default(),
                remote_origin: true,
                sensitive: false,
            });
        }
        Ok(ReadContent {
            content: state.content.clone(),
            remote_origin: state.remote_origin,
            sensitive: state.sensitive,
        })
    }

    fn write(&mut self, content: &ClipboardContent) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.content = content.clone();
        state.remote_origin = true;
        state.sensitive = false;
        state.deferred = None;
        state.writes.push(content.clone());
        for watcher in &state.watchers {
            let _ = watcher.send(BackendEvent::Changed);
        }
        Ok(())
    }

    fn set_fetcher(&mut self, fetcher: Fetcher) {
        self.state.lock().unwrap().fetcher = Some(fetcher);
    }

    fn defer(&mut self, offer: &DeferredOffer) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.deferred = Some(offer.clone());
        state.deferred_offers.push(offer.clone());
        for watcher in &state.watchers {
            let _ = watcher.send(BackendEvent::Changed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_marks_remote_origin_and_notifies() {
        let mut backend = FakeBackend::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        backend.watch(sender).unwrap();
        let content = ClipboardContent {
            text: Some("peer".into()),
            ..Default::default()
        };
        backend.write(&content).unwrap();
        assert!(receiver.try_recv().is_ok());
        let read = backend.read().unwrap();
        assert!(read.remote_origin);
        assert_eq!(read.content, content);
        assert_eq!(backend.writes(), vec![content]);
    }

    #[test]
    fn local_copy_clears_remote_origin() {
        let mut backend = FakeBackend::new();
        backend
            .write(&ClipboardContent {
                text: Some("peer".into()),
                ..Default::default()
            })
            .unwrap();
        backend.set_local(ClipboardContent {
            text: Some("local".into()),
            ..Default::default()
        });
        let read = backend.read().unwrap();
        assert!(!read.remote_origin);
        assert_eq!(read.content.text.as_deref(), Some("local"));
    }
}
