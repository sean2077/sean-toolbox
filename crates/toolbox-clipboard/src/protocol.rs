//! Wire protocol carried inside the Noise transport.

use crate::frame::{Frame, FrameError};
use crate::model::{BlobMeta, Manifest};
use serde::{Deserialize, Serialize};

pub const PROTO_VERSION: u16 = 1;
pub const MAX_MANIFEST_ENTRIES: usize = 4096;
pub const MAX_PATH_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobKind {
    Text,
    Html,
    Image,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        proto_version: u16,
        host_name: String,
    },
    Offer {
        id: String,
        text: Option<BlobMeta>,
        html: Option<BlobMeta>,
        image: Option<BlobMeta>,
        files: Option<Manifest>,
    },
    /// A binary chunk follows in the frame payload. `offset` is the absolute
    /// offset in the concatenated payload for this kind; for files it is the
    /// offset in the concatenation described by the manifest.
    Chunk {
        id: String,
        kind: BlobKind,
        offset: u64,
    },
    End {
        id: String,
        kind: BlobKind,
    },
    Cancel {
        id: String,
    },
    Ping,
    Pong,
}

impl Message {
    pub fn frame(&self) -> Result<Frame, FrameError> {
        Frame::control(self)
    }

    pub fn chunk_frame(&self, data: Vec<u8>) -> Result<Frame, FrameError> {
        Frame::with_data(self, data)
    }

    pub fn from_frame(frame: &Frame) -> Result<Self, FrameError> {
        frame.json()
    }
}

impl Manifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.entries.len() > MAX_MANIFEST_ENTRIES {
            return Err(format!(
                "too many files: {} (max {MAX_MANIFEST_ENTRIES})",
                self.entries.len()
            ));
        }
        let mut total = 0u64;
        let mut seen = std::collections::HashSet::new();
        for entry in &self.entries {
            if !seen.insert(entry.path.as_str()) {
                return Err(format!("duplicate file path: {}", entry.path));
            }
            if entry.path.is_empty() || entry.path.len() > MAX_PATH_BYTES {
                return Err("invalid file path length".into());
            }
            if entry.path.starts_with('/')
                || entry.path.contains('\\')
                || entry.path.contains('\0')
                || entry
                    .path
                    .split('/')
                    .any(|part| part == ".." || part.is_empty())
            {
                return Err(format!("unsafe file path: {}", entry.path));
            }
            total = total
                .checked_add(entry.size)
                .ok_or_else(|| "file size overflow".to_string())?;
        }
        if total != self.total_size {
            return Err("manifest total does not match entries".into());
        }
        Ok(())
    }

    /// Map a concatenated offset to `(entry index, offset within entry)`.
    pub fn locate(&self, offset: u64) -> Option<(usize, u64)> {
        let mut start = 0u64;
        for (index, entry) in self.entries.iter().enumerate() {
            let end = start.checked_add(entry.size)?;
            if offset < end {
                return Some((index, offset - start));
            }
            start = end;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FileEntry;

    #[test]
    fn every_message_round_trips_through_a_frame() {
        let messages = [
            Message::Hello {
                proto_version: PROTO_VERSION,
                host_name: "u26".into(),
            },
            Message::Offer {
                id: "abc".into(),
                text: Some(BlobMeta {
                    size: 3,
                    hash: "00".into(),
                }),
                html: None,
                image: None,
                files: None,
            },
            Message::Chunk {
                id: "abc".into(),
                kind: BlobKind::Text,
                offset: 0,
            },
            Message::End {
                id: "abc".into(),
                kind: BlobKind::Text,
            },
            Message::Cancel { id: "abc".into() },
            Message::Ping,
            Message::Pong,
        ];
        for message in messages {
            let frame = message.frame().unwrap();
            assert_eq!(Message::from_frame(&frame).unwrap(), message);
        }
    }

    #[test]
    fn manifest_locate_maps_concatenated_offsets() {
        let manifest = Manifest {
            entries: vec![
                FileEntry {
                    path: "a".into(),
                    size: 3,
                },
                FileEntry {
                    path: "b/c".into(),
                    size: 2,
                },
            ],
            total_size: 5,
        };
        assert_eq!(manifest.locate(0), Some((0, 0)));
        assert_eq!(manifest.locate(2), Some((0, 2)));
        assert_eq!(manifest.locate(3), Some((1, 0)));
        assert_eq!(manifest.locate(5), None);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_rejects_parent_and_absolute_paths() {
        for path in ["../x", "/etc/passwd", "a\\b", "a//b", ""] {
            let manifest = Manifest {
                entries: vec![FileEntry {
                    path: path.into(),
                    size: 1,
                }],
                total_size: 1,
            };
            assert!(manifest.validate().is_err(), "{path}");
        }
    }
}
