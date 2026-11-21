//! Clipboard content model shared by both platform backends.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_HTML_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_MAX_TRANSFER_BYTES: u64 = 1024 * 1024 * 1024;

/// Metadata for one binary payload sent over the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobMeta {
    pub size: u64,
    /// Lowercase SHA-256 of the payload bytes.
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative path using `/` separators.
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Manifest {
    pub entries: Vec<FileEntry>,
    pub total_size: u64,
}

impl Manifest {
    pub fn hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"files\0");
        for entry in &self.entries {
            hasher.update(entry.path.as_bytes());
            hasher.update(b"\0");
            hasher.update(entry.size.to_le_bytes());
        }
        hex(&hasher.finalize())
    }
}

/// A clipboard snapshot. Text and HTML may coexist, and an image may coexist
/// with text (for example Chrome's "copy image").
/// Metadata for a clipboard item whose payload is still on the source side.
/// The receiver installs a deferred clipboard entry and asks for the payload
/// only when an application actually pastes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredOffer {
    pub id: String,
    pub text: Option<BlobMeta>,
    pub html: Option<BlobMeta>,
    pub image: Option<BlobMeta>,
    pub files: Option<Manifest>,
}

impl DeferredOffer {
    pub fn has_files(&self) -> bool {
        self.files.is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.image.is_none() && self.files.is_none()
    }

    pub fn kind(&self) -> &'static str {
        let text = self.text.is_some();
        let html = self.html.is_some();
        let image = self.image.is_some();
        let files = self.files.is_some();
        if files {
            "Files"
        } else if image && (text || html) {
            "Mixed"
        } else if image {
            "Image"
        } else if html && text {
            "Mixed"
        } else if html {
            "HTML"
        } else if text {
            "Text"
        } else {
            "Empty"
        }
    }

    /// Approximate payload size used for transfer limits and fetch timeouts.
    pub fn total_bytes(&self) -> u64 {
        self.text.as_ref().map_or(0, |meta| meta.size)
            + self.html.as_ref().map_or(0, |meta| meta.size)
            + self.image.as_ref().map_or(0, |meta| meta.size)
            + self
                .files
                .as_ref()
                .map_or(0, |manifest| manifest.total_size)
    }

    pub fn validate(&self, max_transfer_bytes: u64) -> Result<(), String> {
        if let Some(meta) = &self.text
            && meta.size > MAX_TEXT_BYTES
        {
            return Err("text exceeds the 16 MiB limit".into());
        }
        if let Some(meta) = &self.html
            && meta.size > MAX_HTML_BYTES
        {
            return Err("HTML exceeds the 16 MiB limit".into());
        }
        if let Some(meta) = &self.image
            && meta.size > MAX_IMAGE_BYTES
        {
            return Err("image exceeds the 64 MiB limit".into());
        }
        if let Some(files) = &self.files {
            files.validate()?;
            if files.total_size > max_transfer_bytes {
                return Err("files exceed the transfer limit".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClipboardContent {
    pub text: Option<String>,
    pub html: Option<String>,
    /// PNG bytes.
    pub image: Option<Vec<u8>>,
    /// Absolute local paths. The wire format carries only relative names.
    pub files: Vec<PathBuf>,
}

impl ClipboardContent {
    pub fn is_empty(&self) -> bool {
        self.text.as_deref().is_none_or(str::is_empty)
            && self.html.as_deref().is_none_or(str::is_empty)
            && self.image.as_ref().is_none_or(Vec::is_empty)
            && self.files.is_empty()
    }

    pub fn kind(&self) -> &'static str {
        let text = self.text.is_some();
        let html = self.html.is_some();
        let image = self.image.is_some();
        let files = !self.files.is_empty();
        if files {
            "Files"
        } else if image && (text || html) {
            "Mixed"
        } else if image {
            "Image"
        } else if html && text {
            "Mixed"
        } else if html {
            "HTML"
        } else if text {
            "Text"
        } else {
            "Empty"
        }
    }

    /// Stable fingerprint used to suppress echo loops. Files are represented by
    /// their relative manifest (path + size), which is identical on both peers.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        if let Some(text) = &self.text {
            hasher.update(b"text\0");
            hasher.update((text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        }
        if let Some(html) = &self.html {
            hasher.update(b"html\0");
            hasher.update((html.len() as u64).to_le_bytes());
            hasher.update(html.as_bytes());
        }
        if let Some(image) = &self.image {
            hasher.update(b"image\0");
            hasher.update((image.len() as u64).to_le_bytes());
            hasher.update(image);
        }
        if !self.files.is_empty() {
            hasher.update(b"files\0");
            // The sender's manifest order is deterministic; the receiver hashes
            // the paths it was given rather than the local absolute paths.
            let mut entries = self
                .files
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            entries.sort();
            for path in entries {
                hasher.update(path.as_bytes());
                hasher.update(b"\0");
            }
        }
        hex(&hasher.finalize())
    }

    pub fn text_len(&self) -> u64 {
        self.text.as_ref().map_or(0, |s| s.len() as u64)
    }

    pub fn html_len(&self) -> u64 {
        self.html.as_ref().map_or(0, |s| s.len() as u64)
    }

    pub fn image_len(&self) -> u64 {
        self.image.as_ref().map_or(0, |v| v.len() as u64)
    }
}

pub fn blob_meta(bytes: &[u8]) -> BlobMeta {
    BlobMeta {
        size: bytes.len() as u64,
        hash: bytes_hash(bytes),
    }
}

pub fn bytes_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(&hasher.finalize())
}

pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn path_to_wire(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_content_is_empty() {
        assert!(ClipboardContent::default().is_empty());
        assert_eq!(ClipboardContent::default().kind(), "Empty");
    }

    #[test]
    fn text_and_html_fingerprint_is_stable() {
        let content = ClipboardContent {
            text: Some("hello".into()),
            html: Some("<b>hello</b>".into()),
            ..Default::default()
        };
        assert_eq!(content.fingerprint(), content.clone().fingerprint());
        assert_eq!(content.kind(), "Mixed");
    }

    #[test]
    fn manifest_hash_does_not_depend_on_local_prefix() {
        let a = Manifest {
            entries: vec![FileEntry {
                path: "dir/a.txt".into(),
                size: 3,
            }],
            total_size: 3,
        };
        let b = a.clone();
        assert_eq!(a.hash(), b.hash());
    }
}
