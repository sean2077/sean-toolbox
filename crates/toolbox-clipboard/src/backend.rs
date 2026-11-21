//! Platform backend boundary and an in-memory backend for tests.

use crate::model::ClipboardContent;
use std::sync::{Arc, Mutex, mpsc::Sender};

pub const ORIGIN_MIME: &str = "application/x-sean-toolbox-origin";
pub const ORIGIN_WINDOWS_FORMAT: &str = "SeanToolbox.ClipOrigin";

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
}

#[derive(Debug, Default)]
struct FakeState {
    content: ClipboardContent,
    remote_origin: bool,
    sensitive: bool,
    watchers: Vec<Sender<BackendEvent>>,
    writes: Vec<ClipboardContent>,
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
        for watcher in &state.watchers {
            let _ = watcher.send(BackendEvent::Changed);
        }
    }

    pub fn set_sensitive_local(&self, content: ClipboardContent) {
        let mut state = self.state.lock().unwrap();
        state.content = content;
        state.remote_origin = false;
        state.sensitive = true;
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
        state.writes.push(content.clone());
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
