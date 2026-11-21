//! Platform backend selection.

use crate::backend::Backend;

#[cfg(target_os = "linux")]
pub fn backend() -> Result<Box<dyn Backend>, String> {
    crate::linux::backend()
}

#[cfg(windows)]
pub fn backend() -> Result<Box<dyn Backend>, String> {
    crate::windows::backend()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub fn backend() -> Result<Box<dyn Backend>, String> {
    Err("clipboard sync is supported on Windows and Linux only".into())
}
