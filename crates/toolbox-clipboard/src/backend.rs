//! Platform backend boundary and an in-memory backend for tests.
//!
//! Text, HTML and images are written to the local clipboard as soon as they
//! arrive. Files are different: the peer only announces them, and the backend
//! installs a clipboard entry whose file list is produced when an application
//! pastes. The backend asks the service for the staged files through a
//! [`Fetcher`].

use crate::model::ClipboardContent;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Sender},
};
use std::time::Duration;

pub const ORIGIN_MIME: &str = "application/x-sean-toolbox-origin";
pub const ORIGIN_WINDOWS_FORMAT: &str = "SeanToolbox.ClipOrigin";

/// How long a paste waits for announced files. Qt gives up on a clipboard pipe
/// after one second without data, so the answer must come before that. Files
/// that take longer keep downloading and the next paste uses the local copy.
pub const PASTE_WAIT: Duration = Duration::from_millis(800);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The transfer is running; paste again when it finishes.
    InProgress,
    /// The files cannot be provided (peer offline, newer copy, source changed).
    Failed(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InProgress => f.write_str("files are still being transferred"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

pub type FetchResult = Result<Vec<PathBuf>, FetchError>;

/// A paste asked for the files of announced item `id`.
pub struct FetchRequest {
    pub id: String,
    pub reply: Sender<FetchResult>,
}

/// Lets a backend ask the service for the files of an announced item.
#[derive(Clone)]
pub struct Fetcher {
    send: Arc<dyn Fn(FetchRequest) -> bool + Send + Sync>,
}

impl Fetcher {
    pub fn new(send: impl Fn(FetchRequest) -> bool + Send + Sync + 'static) -> Self {
        Self {
            send: Arc::new(send),
        }
    }

    /// Return the staged top-level paths, waiting at most `wait`.
    pub fn fetch(&self, id: &str, wait: Duration) -> FetchResult {
        let (reply, response) = mpsc::channel();
        if !(self.send)(FetchRequest {
            id: id.to_owned(),
            reply,
        }) {
            return Err(FetchError::Failed("clipboard service stopped".into()));
        }
        match response.recv_timeout(wait) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(FetchError::InProgress),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(FetchError::Failed("clipboard service stopped".into()))
            }
        }
    }
}

impl std::fmt::Debug for Fetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Fetcher")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadContent {
    pub content: ClipboardContent,
    /// The entry was written by this bridge (origin marker present).
    pub remote_origin: bool,
    /// A password manager marked the entry as secret.
    pub sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendEvent {
    Changed,
    /// A queued write or file entry could not be put on the clipboard
    /// (usually another program kept it open).
    WriteFailed,
    Unavailable,
}

pub trait Backend: Send {
    fn watch(&mut self, events: Sender<BackendEvent>) -> Result<(), String>;
    fn read(&mut self) -> Result<ReadContent, String>;
    /// Write text, HTML and/or an image received from the peer.
    fn write(&mut self, content: &ClipboardContent) -> Result<(), String>;
    /// Install an entry for files announced by the peer. The file list is
    /// produced through `fetcher` only when an application pastes.
    fn offer_files(&mut self, id: &str, fetcher: Fetcher) -> Result<(), String>;
}

#[derive(Default)]
struct FakeState {
    content: ClipboardContent,
    remote_origin: bool,
    sensitive: bool,
    watchers: Vec<Sender<BackendEvent>>,
    writes: Vec<ClipboardContent>,
    offered: Option<(String, Fetcher)>,
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

    fn changed(state: &FakeState) {
        for watcher in &state.watchers {
            let _ = watcher.send(BackendEvent::Changed);
        }
    }

    /// Simulate a local copy by the user.
    pub fn set_local(&self, content: ClipboardContent) {
        let mut state = self.state.lock().unwrap();
        state.content = content;
        state.remote_origin = false;
        state.sensitive = false;
        state.offered = None;
        Self::changed(&state);
    }

    pub fn set_sensitive(&self, content: ClipboardContent) {
        let mut state = self.state.lock().unwrap();
        state.content = content;
        state.remote_origin = false;
        state.sensitive = true;
        state.offered = None;
        Self::changed(&state);
    }

    pub fn writes(&self) -> Vec<ClipboardContent> {
        self.state.lock().unwrap().writes.clone()
    }

    pub fn clear_writes(&self) {
        self.state.lock().unwrap().writes.clear();
    }

    /// Id of the announced files currently on the fake clipboard.
    pub fn offered_id(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .offered
            .as_ref()
            .map(|(id, _)| id.clone())
    }

    /// Simulate an application pasting the announced files.
    pub fn paste_files(&self, wait: Duration) -> FetchResult {
        let offered = self.state.lock().unwrap().offered.clone();
        let Some((id, fetcher)) = offered else {
            return Err(FetchError::Failed("no files on the clipboard".into()));
        };
        fetcher.fetch(&id, wait)
    }
}

impl Backend for FakeBackend {
    fn watch(&mut self, events: Sender<BackendEvent>) -> Result<(), String> {
        self.state.lock().unwrap().watchers.push(events);
        Ok(())
    }

    fn read(&mut self) -> Result<ReadContent, String> {
        let state = self.state.lock().unwrap();
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
        state.offered = None;
        state.writes.push(content.clone());
        Self::changed(&state);
        Ok(())
    }

    fn offer_files(&mut self, id: &str, fetcher: Fetcher) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        state.content = ClipboardContent::default();
        state.remote_origin = true;
        state.sensitive = false;
        state.offered = Some((id.to_owned(), fetcher));
        Self::changed(&state);
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

    #[test]
    fn fetch_times_out_as_in_progress() {
        let held = Arc::new(Mutex::new(Vec::new()));
        let keep = held.clone();
        let fetcher = Fetcher::new(move |request| {
            keep.lock().unwrap().push(request.reply);
            true
        });
        assert_eq!(
            fetcher.fetch("id", Duration::from_millis(20)),
            Err(FetchError::InProgress)
        );
    }
}
