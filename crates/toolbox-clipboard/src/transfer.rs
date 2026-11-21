//! File manifests, receiver staging and path safety.

use crate::model::{FileEntry, Manifest};
use std::{
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Debug)]
pub enum TransferError {
    Io(std::io::Error),
    TooLarge(u64),
    UnsafePath(String),
    Invalid(String),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O: {error}"),
            Self::TooLarge(size) => write!(f, "transfer too large: {size} bytes"),
            Self::UnsafePath(path) => write!(f, "unsafe path: {path}"),
            Self::Invalid(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for TransferError {}

impl From<std::io::Error> for TransferError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

/// Expand the selected files and directories without following symlinks.
pub fn collect_manifest(paths: &[PathBuf], max_bytes: u64) -> Result<Manifest, TransferError> {
    if paths.is_empty() {
        return Err(TransferError::Invalid("no files selected".into()));
    }
    let mut entries = Vec::new();
    let mut total = 0u64;
    for path in paths {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let root_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| TransferError::UnsafePath(path.display().to_string()))?;
        if metadata.is_file() {
            total = checked_add(total, metadata.len())?;
            entries.push(FileEntry {
                path: root_name.to_owned(),
                size: metadata.len(),
            });
        } else if metadata.is_dir() {
            collect_dir(path, Path::new(root_name), &mut entries, &mut total)?;
        }
        if total > max_bytes {
            return Err(TransferError::TooLarge(total));
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest = Manifest {
        entries,
        total_size: total,
    };
    manifest.validate().map_err(TransferError::Invalid)?;
    Ok(manifest)
}

fn collect_dir(
    directory: &Path,
    relative: &Path,
    entries: &mut Vec<FileEntry>,
    total: &mut u64,
) -> Result<(), TransferError> {
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        let metadata = fs::symlink_metadata(child.path())?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        let name = child
            .file_name()
            .into_string()
            .map_err(|_| TransferError::UnsafePath(child.path().display().to_string()))?;
        let next = relative.join(name);
        if metadata.is_dir() {
            collect_dir(&child.path(), &next, entries, total)?;
        } else if metadata.is_file() {
            *total = checked_add(*total, metadata.len())?;
            entries.push(FileEntry {
                path: wire_path(&next),
                size: metadata.len(),
            });
        }
    }
    Ok(())
}

fn wire_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn checked_add(left: u64, right: u64) -> Result<u64, TransferError> {
    left.checked_add(right)
        .ok_or(TransferError::TooLarge(u64::MAX))
}

/// Resolve a wire path below `root`, rejecting traversal, absolute paths,
/// drive letters, UNC paths, reserved names and Windows-invalid characters.
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, TransferError> {
    if relative.is_empty() || relative.starts_with('/') || relative.starts_with("//") {
        return Err(TransferError::UnsafePath(relative.into()));
    }
    if relative.contains('\\')
        || relative.contains('\0')
        || relative.contains(':')
        || relative.contains('<')
        || relative.contains('>')
        || relative.contains('"')
        || relative.contains('|')
        || relative.contains('?')
        || relative.contains('*')
    {
        return Err(TransferError::UnsafePath(relative.into()));
    }
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(TransferError::UnsafePath(relative.into()));
        }
        if part.ends_with(' ') || part.ends_with('.') || is_reserved(part) {
            return Err(TransferError::UnsafePath(relative.into()));
        }
        path.push(part);
    }
    Ok(path)
}

fn is_reserved(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// Return an unused path, adding `.1`, `.2`, ... before the extension.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().into_owned());
    for index in 1..=10_000 {
        let name = match &extension {
            Some(extension) => format!("{stem}.{index}.{extension}"),
            None => format!("{stem}.{index}"),
        };
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}.{}", std::process::id()))
}

pub fn cache_root() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return PathBuf::from(local).join("sean-toolbox").join("clipboard");
        }
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(cache).join("sean-toolbox").join("clipboard");
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join("Library/Caches/sean-toolbox/clipboard");
        }
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".cache/sean-toolbox/clipboard"))
        .unwrap_or_else(|| std::env::temp_dir().join("sean-toolbox-clipboard"))
}

/// Keep the newest `keep` transfer directories and remove anything older than
/// `max_age`. Called at startup and after a completed receive.
pub fn cleanup_cache(root: &Path, keep: usize, max_age: Duration) -> Result<(), TransferError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut items = Vec::new();
    let now = SystemTime::now();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let modified = entry.metadata()?.modified().unwrap_or(now);
        let age = now.duration_since(modified).unwrap_or_default();
        items.push((modified, age, entry.path()));
    }
    items.sort_by_key(|(modified, _, _)| *modified);
    let excess = items.len().saturating_sub(keep);
    for (index, (_, age, path)) in items.into_iter().enumerate() {
        if index < excess || age > max_age {
            let _ = fs::remove_dir_all(path);
        }
    }
    Ok(())
}

/// Writes chunks into a staged directory and atomically promotes it on finish.
pub struct FileSink {
    temp: PathBuf,
    final_path: PathBuf,
    files: Vec<(PathBuf, File, u64)>,
}

impl FileSink {
    pub fn create(root: &Path, id: &str, manifest: &Manifest) -> Result<Self, TransferError> {
        manifest.validate().map_err(TransferError::Invalid)?;
        fs::create_dir_all(root)?;
        let final_path = unique_path(&root.join(id));
        let temp = root.join(format!("{id}.tmp"));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp)?;
        let mut files = Vec::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            let path = safe_join(&temp, &entry.path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = File::create(&path)?;
            files.push((path, file, entry.size));
        }
        Ok(Self {
            temp,
            final_path,
            files,
        })
    }

    pub fn write_chunk(
        &mut self,
        manifest: &Manifest,
        offset: u64,
        data: &[u8],
    ) -> Result<(), TransferError> {
        let (index, within) = manifest
            .locate(offset)
            .ok_or_else(|| TransferError::Invalid("chunk offset outside manifest".into()))?;
        let (_, file, size) = self
            .files
            .get_mut(index)
            .ok_or_else(|| TransferError::Invalid("file index outside manifest".into()))?;
        if within + data.len() as u64 > *size {
            return Err(TransferError::Invalid("chunk exceeds file size".into()));
        }
        file.seek(SeekFrom::Start(within))?;
        file.write_all(data)?;
        Ok(())
    }

    pub fn finish(mut self) -> Result<PathBuf, TransferError> {
        for (_, file, _) in &mut self.files {
            file.flush()?;
        }
        drop(self.files);
        fs::rename(&self.temp, &self.final_path)?;
        Ok(self.final_path)
    }

    pub fn cancel(self) {
        drop(self.files);
        let _ = fs::remove_dir_all(&self.temp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_rejects_traversal_and_windows_escapes() {
        let root = Path::new("/tmp/root");
        for bad in [
            "../x",
            "/etc/passwd",
            "C:/x",
            "//server/share",
            "a\\b",
            "CON",
            "a//b",
            "a/./b",
            ".hidden/..",
            "bad:name",
            "name.",
        ] {
            assert!(safe_join(root, bad).is_err(), "{bad}");
        }
        assert!(safe_join(root, "dir/中文.txt").is_ok());
    }

    #[test]
    fn manifest_collects_files_and_rejects_large_totals() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/a.txt"), b"abc").unwrap();
        fs::write(dir.path().join("b.txt"), b"de").unwrap();
        let manifest = collect_manifest(&[dir.path().to_path_buf()], 1024).unwrap();
        assert_eq!(manifest.total_size, 5);
        assert_eq!(manifest.entries.len(), 2);
        assert!(collect_manifest(&[dir.path().to_path_buf()], 4).is_err());
    }

    #[test]
    fn sink_writes_and_promotes_files() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            entries: vec![FileEntry {
                path: "a.txt".into(),
                size: 3,
            }],
            total_size: 3,
        };
        let mut sink = FileSink::create(dir.path(), "id1", &manifest).unwrap();
        sink.write_chunk(&manifest, 0, b"abc").unwrap();
        let path = sink.finish().unwrap();
        assert_eq!(fs::read(path.join("a.txt")).unwrap(), b"abc");
    }

    #[test]
    fn sink_rejects_out_of_range_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            entries: vec![FileEntry {
                path: "a".into(),
                size: 2,
            }],
            total_size: 2,
        };
        let mut sink = FileSink::create(dir.path(), "id2", &manifest).unwrap();
        assert!(sink.write_chunk(&manifest, 1, b"xx").is_err());
        sink.cancel();
    }

    #[test]
    fn unique_path_adds_a_counter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, b"x").unwrap();
        assert_eq!(unique_path(&path), dir.path().join("a.1.txt"));
    }
}
