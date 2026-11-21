//! Cross-machine clipboard bridge for Sean Toolbox.

pub mod backend;
pub mod cf_html;
pub mod crypto;
pub mod frame;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod model;
pub mod platform;
pub mod protocol;
pub mod service;
pub mod sync;
pub mod transfer;
#[cfg(windows)]
pub mod windows;

pub use backend::{Backend, BackendEvent, ReadContent};
pub use model::{BlobMeta, ClipboardContent, DEFAULT_MAX_TRANSFER_BYTES, FileEntry, Manifest};
pub use protocol::{BlobKind, Message, PROTO_VERSION};
pub use service::{Handle, Role, Settings, Status, default_cache_dir};
