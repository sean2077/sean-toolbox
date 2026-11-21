//! Length-prefixed frames. A frame carries a JSON metadata object and an
//! optional binary payload (used for file/image chunks).

use serde::{Serialize, de::DeserializeOwned};
use std::io::{Read, Write};

/// Noise transport messages are limited to 65535 bytes including the 16-byte
/// tag; keep plaintext frames comfortably below that.
pub const MAX_FRAME_BYTES: usize = 60 * 1024;
pub const CHUNK_BYTES: usize = 32 * 1024;
const _: () = assert!(CHUNK_BYTES + 1024 < MAX_FRAME_BYTES);

#[derive(Debug)]
pub enum FrameError {
    Io(std::io::Error),
    Json(serde_json::Error),
    TooLarge(usize),
    Invalid(&'static str),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "frame I/O: {error}"),
            Self::Json(error) => write!(f, "frame JSON: {error}"),
            Self::TooLarge(size) => write!(f, "frame too large: {size} bytes"),
            Self::Invalid(reason) => write!(f, "invalid frame: {reason}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<std::io::Error> for FrameError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for FrameError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub meta: serde_json::Value,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn control<T: Serialize>(message: &T) -> Result<Self, FrameError> {
        Ok(Self {
            meta: serde_json::to_value(message)?,
            data: Vec::new(),
        })
    }

    pub fn with_data<T: Serialize>(message: &T, data: Vec<u8>) -> Result<Self, FrameError> {
        Ok(Self {
            meta: serde_json::to_value(message)?,
            data,
        })
    }

    pub fn json<T: DeserializeOwned>(&self) -> Result<T, FrameError> {
        Ok(serde_json::from_value(self.meta.clone())?)
    }

    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let meta = serde_json::to_vec(&self.meta)?;
        let total = meta
            .len()
            .checked_add(self.data.len())
            .ok_or(FrameError::TooLarge(usize::MAX))?;
        let size = total
            .checked_add(8)
            .ok_or(FrameError::TooLarge(usize::MAX))?;
        if size > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(size));
        }
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        out.extend_from_slice(&meta);
        out.extend_from_slice(&self.data);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        if bytes.len() < 8 {
            return Err(FrameError::Invalid("missing header"));
        }
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(bytes.len()));
        }
        let total = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
        let meta_len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        if total + 8 > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(total + 8));
        }
        if total > bytes.len() - 8 || meta_len > total {
            return Err(FrameError::Invalid("length mismatch"));
        }
        let meta_end = 8 + meta_len;
        let data_end = 8 + total;
        let meta = serde_json::from_slice(&bytes[8..meta_end])?;
        Ok(Self {
            meta,
            data: bytes[meta_end..data_end].to_vec(),
        })
    }

    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self, FrameError> {
        let mut header = [0u8; 8];
        reader.read_exact(&mut header)?;
        let total = u32::from_be_bytes(header[0..4].try_into().unwrap()) as usize;
        let meta_len = u32::from_be_bytes(header[4..8].try_into().unwrap()) as usize;
        let size = 8usize
            .checked_add(total)
            .ok_or(FrameError::TooLarge(usize::MAX))?;
        if size > MAX_FRAME_BYTES {
            return Err(FrameError::TooLarge(size));
        }
        if meta_len > total {
            return Err(FrameError::Invalid("metadata length exceeds frame"));
        }
        let mut meta = vec![0u8; meta_len];
        reader.read_exact(&mut meta)?;
        let mut data = vec![0u8; total - meta_len];
        reader.read_exact(&mut data)?;
        Ok(Self {
            meta: serde_json::from_slice(&meta)?,
            data,
        })
    }

    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<(), FrameError> {
        writer.write_all(&self.encode()?)?;
        writer.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
    struct Sample {
        id: u64,
        name: String,
    }

    #[test]
    fn control_frame_round_trips() {
        let frame = Frame::control(&Sample {
            id: 7,
            name: "中文".into(),
        })
        .unwrap();
        let bytes = frame.encode().unwrap();
        assert_eq!(Frame::decode(&bytes).unwrap(), frame);
    }

    #[test]
    fn binary_frame_round_trips_through_reader() {
        let frame = Frame::with_data(
            &Sample {
                id: 1,
                name: "data".into(),
            },
            vec![0, 1, 2, 255],
        )
        .unwrap();
        let bytes = frame.encode().unwrap();
        let mut cursor = std::io::Cursor::new(bytes);
        assert_eq!(Frame::read_from(&mut cursor).unwrap(), frame);
    }

    #[test]
    fn truncated_and_oversized_frames_are_rejected() {
        assert!(Frame::decode(&[0, 0, 0, 1]).is_err());
        let frame = Frame::control(&Sample {
            id: 1,
            name: "x".into(),
        })
        .unwrap();
        let mut bytes = frame.encode().unwrap();
        bytes[0..4].copy_from_slice(&(MAX_FRAME_BYTES as u32).to_be_bytes());
        assert!(matches!(
            Frame::decode(&bytes),
            Err(FrameError::TooLarge(_))
        ));
        let mut bytes = frame.encode().unwrap();
        bytes[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(Frame::decode(&bytes).is_err());
    }
}
