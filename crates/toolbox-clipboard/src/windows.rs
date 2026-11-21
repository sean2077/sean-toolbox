//! Windows clipboard backend.
//!
//! Uses a message-only window with `AddClipboardFormatListener`, CF_HTML,
//! PNG/CF_DIB and CF_HDROP. This file is filled in after the protocol and
//! service tests pass.

use crate::backend::Backend;

pub fn backend() -> Result<Box<dyn Backend>, String> {
    Err("Windows clipboard backend is not wired yet".into())
}
