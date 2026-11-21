//! Host-side lifecycle for the clipboard bridge.

use std::path::{Path, PathBuf};
use toolbox_clipboard::{Handle, Role, Settings, Status};
use toolbox_core::{ClipboardStatus, ClipboardSyncConfig, ClipboardTransfer};

pub struct ClipboardWorker {
    config_path: PathBuf,
    current: Option<ClipboardSyncConfig>,
    handle: Option<Handle>,
    last_error: Option<String>,
    reported_error: bool,
}

impl ClipboardWorker {
    pub fn new(config_path: &Path, config: &ClipboardSyncConfig) -> Self {
        let mut worker = Self {
            config_path: config_path.to_owned(),
            current: None,
            handle: None,
            last_error: None,
            reported_error: false,
        };
        worker.apply(config);
        worker
    }

    /// Start, stop or restart the bridge when the clipboard section changes.
    /// Failures are reported as warnings; the host keeps running.
    pub fn apply(&mut self, config: &ClipboardSyncConfig) {
        if self.current.as_ref() == Some(config) && self.handle.is_some() {
            return;
        }
        self.stop();
        self.current = Some(config.clone());
        self.last_error = None;
        self.reported_error = false;
        if !config.enabled {
            return;
        }
        let role = match config.role {
            toolbox_core::ClipboardRole::Listen => Role::Listen,
            toolbox_core::ClipboardRole::Connect => Role::Connect,
        };
        let settings = Settings {
            role,
            listen: config.listen.clone(),
            peer: config.peer.clone(),
            max_transfer_bytes: config.max_transfer_bytes,
            key_path: self.config_path.with_file_name("clipboard.key"),
            cache_dir: toolbox_clipboard::default_cache_dir(),
            host_name: host_name(),
            silence_timeout: std::time::Duration::from_secs(45),
        };
        match toolbox_clipboard::start(settings) {
            Ok(handle) => self.handle = Some(handle),
            Err(error) => self.last_error = Some(error),
        }
    }

    pub fn stop(&mut self) {
        if let Some(mut handle) = self.handle.take() {
            handle.stop();
        }
    }

    pub fn status(&self) -> Option<ClipboardStatus> {
        let config = self.current.as_ref()?;
        if !config.enabled {
            return None;
        }
        let status = self
            .handle
            .as_ref()
            .map(Handle::status)
            .unwrap_or_else(|| Status {
                enabled: true,
                role: match config.role {
                    toolbox_core::ClipboardRole::Listen => "listen".into(),
                    toolbox_core::ClipboardRole::Connect => "connect".into(),
                },
                peer: if matches!(config.role, toolbox_core::ClipboardRole::Listen) {
                    config.listen.clone()
                } else {
                    config.peer.clone()
                },
                last_error: self.last_error.clone(),
                ..Default::default()
            });
        Some(ClipboardStatus {
            enabled: status.enabled,
            role: status.role,
            connected: status.connected,
            peer: status.peer,
            last_sent: status.last_sent,
            last_received: status.last_received,
            sent_count: status.sent_count,
            received_count: status.received_count,
            transfer: status.transfer.map(|transfer| ClipboardTransfer {
                receiving: transfer.receiving,
                done: transfer.done,
                total: transfer.total,
            }),
            cache_bytes: status.cache_bytes,
            last_error: status.last_error.or_else(|| self.last_error.clone()),
        })
    }

    /// Delete received files. Without a running bridge the cache directory is
    /// cleared directly.
    pub fn clear_cache(&self) {
        match &self.handle {
            Some(handle) => handle.clear_cache(),
            None => {
                toolbox_clipboard::transfer::clear_cache(
                    &toolbox_clipboard::default_cache_dir(),
                    None,
                );
            }
        }
    }

    pub fn take_warnings(&mut self) -> Vec<String> {
        if self.reported_error {
            return Vec::new();
        }
        match &self.last_error {
            Some(error) => {
                self.reported_error = true;
                vec![format!("Clipboard: {error}")]
            }
            None => Vec::new(),
        }
    }
}

impl Drop for ClipboardWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn host_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "sean-toolbox".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use toolbox_core::{ClipboardRole, ClipboardSyncConfig};

    #[test]
    fn disabled_clipboard_has_no_status_and_no_warning() {
        let dir = tempfile::tempdir().unwrap();
        let mut worker = ClipboardWorker::new(
            &dir.path().join("config.json"),
            &ClipboardSyncConfig::default(),
        );
        assert!(worker.status().is_none());
        assert!(worker.take_warnings().is_empty());
    }

    #[test]
    fn missing_key_is_reported_without_stopping_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let config = ClipboardSyncConfig {
            enabled: true,
            role: ClipboardRole::Listen,
            listen: "127.0.0.1:24802".into(),
            peer: String::new(),
            max_transfer_bytes: 1024,
            ..Default::default()
        };
        let mut worker = ClipboardWorker::new(&dir.path().join("config.json"), &config);
        let warnings = worker.take_warnings();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.starts_with("Clipboard:"))
        );
        let status = worker.status().unwrap();
        assert!(status.enabled);
        assert!(status.last_error.is_some());
    }
}
