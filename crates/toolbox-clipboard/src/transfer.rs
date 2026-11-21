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
/// Receive cache limits. Files only pass through the cache on their way to the
/// paste target, but a path pasted into a terminal or Claude Code may be read
/// later, so items live for a day unless the total grows past the cap.
pub const CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Apply the cache limits and return the bytes left in `root`.
///
/// * Interrupted staging directories (`*.tmp`) are removed, except `active`.
/// * Items older than `max_age` are removed, except `keep` (the item on the
///   clipboard right now).
/// * The oldest items are removed while the total exceeds `max_bytes`; `keep`
///   is removed last and only if it alone exceeds the cap.
pub fn cleanup_cache(
    root: &Path,
    max_bytes: u64,
    max_age: Duration,
    keep: Option<&Path>,
    active: Option<&Path>,
) -> u64 {
    let now = SystemTime::now();
    let mut items = Vec::new();
    let mut active_bytes = 0;
    for (path, modified) in cache_entries(root) {
        if Some(path.as_path()) == active {
            active_bytes = tree_size(&path);
            continue;
        }
        let is_temp = path.extension().is_some_and(|extension| extension == "tmp");
        let kept = Some(path.as_path()) == keep;
        let expired = now.duration_since(modified).unwrap_or_default() > max_age;
        if is_temp || (expired && !kept) {
            let _ = fs::remove_dir_all(&path);
            continue;
        }
        items.push((kept, modified, tree_size(&path), path));
    }
    // Oldest first, the kept item last.
    items.sort_by_key(|(kept, modified, _, _)| (*kept, *modified));
    let mut total = active_bytes + items.iter().map(|(_, _, size, _)| size).sum::<u64>();
    for (_, _, size, path) in &items {
        if total <= max_bytes {
            break;
        }
        if fs::remove_dir_all(path).is_ok() {
            total -= size;
        }
    }
    total
}

/// Remove every cached item except the one still being received.
pub fn clear_cache(root: &Path, active: Option<&Path>) -> u64 {
    let mut left = 0;
    for (path, _) in cache_entries(root) {
        if Some(path.as_path()) == active {
            left += tree_size(&path);
        } else {
            let _ = fs::remove_dir_all(&path);
        }
    }
    left
}

fn cache_entries(root: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| {
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or_else(|_| SystemTime::now());
            (entry.path(), modified)
        })
        .collect()
}

/// Total size of regular files below `path`, without following symlinks.
pub fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    if !metadata.is_dir() {
        return 0;
    }
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| tree_size(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// Top-level names of a manifest: the files and folders the user selected.
pub fn top_level_names(manifest: &Manifest) -> Vec<String> {
    let mut names = Vec::new();
    for entry in &manifest.entries {
        let name = entry.path.split('/').next().unwrap_or_default().to_owned();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// Writes chunks into a staging directory and renames it into place when the
/// last chunk arrives. Files are opened one at a time, so a large folder does
/// not exhaust file descriptors.
pub struct FileSink {
    temp: PathBuf,
    final_path: PathBuf,
    paths: Vec<PathBuf>,
    open: Option<(usize, File)>,
}

impl FileSink {
    pub fn create(root: &Path, id: &str, manifest: &Manifest) -> Result<Self, TransferError> {
        manifest.validate().map_err(TransferError::Invalid)?;
        fs::create_dir_all(root)?;
        let final_path = unique_path(&root.join(id));
        let temp = root.join(format!("{id}.tmp"));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp)?;
        let paths = manifest
            .entries
            .iter()
            .map(|entry| safe_join(&temp, &entry.path))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            temp,
            final_path,
            paths,
            open: None,
        })
    }

    /// The staging directory, for cache cleanup.
    pub fn temp_dir(&self) -> &Path {
        &self.temp
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
        let size = manifest.entries[index].size;
        if within + data.len() as u64 > size {
            return Err(TransferError::Invalid("chunk exceeds file size".into()));
        }
        if self.open.as_ref().is_none_or(|(open, _)| *open != index) {
            let path = &self.paths[index];
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path)?;
            self.open = Some((index, file));
        }
        let (_, file) = self.open.as_mut().expect("opened above");
        file.seek(SeekFrom::Start(within))?;
        file.write_all(data)?;
        Ok(())
    }

    /// Create any file that received no chunk (empty files), then move the
    /// staging directory into place and return the selected top-level paths.
    pub fn finish(mut self, manifest: &Manifest) -> Result<Vec<PathBuf>, TransferError> {
        if let Some((_, mut file)) = self.open.take() {
            file.flush()?;
        }
        for path in &self.paths {
            if !path.exists() {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                File::create(path)?;
            }
        }
        fs::rename(&self.temp, &self.final_path)?;
        Ok(top_level_names(manifest)
            .into_iter()
            .map(|name| self.final_path.join(name))
            .collect())
    }

    pub fn cancel(mut self) {
        self.open.take();
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
        let paths = sink.finish(&manifest).unwrap();
        assert_eq!(paths.len(), 1);
        assert_eq!(fs::read(&paths[0]).unwrap(), b"abc");
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

    #[test]
    fn sink_creates_empty_files_and_returns_top_level_paths() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = Manifest {
            entries: vec![
                FileEntry {
                    path: "目录/a".into(),
                    size: 2,
                },
                FileEntry {
                    path: "目录/empty".into(),
                    size: 0,
                },
                FileEntry {
                    path: "b".into(),
                    size: 1,
                },
            ],
            total_size: 3,
        };
        let mut sink = FileSink::create(dir.path(), "id3", &manifest).unwrap();
        sink.write_chunk(&manifest, 0, b"xy").unwrap();
        sink.write_chunk(&manifest, 2, b"z").unwrap();
        let paths = sink.finish(&manifest).unwrap();
        let root = dir.path().join("id3");
        assert_eq!(paths, vec![root.join("目录"), root.join("b")]);
        assert_eq!(fs::read(root.join("目录/empty")).unwrap(), b"");
    }

    fn item(root: &Path, name: &str, bytes: usize, age: Duration) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("data"), vec![0u8; bytes]).unwrap();
        let time = SystemTime::now() - age;
        let mut options = fs::OpenOptions::new();
        options.read(true);
        // A directory handle on Windows needs FILE_FLAG_BACKUP_SEMANTICS and
        // write access to change its timestamps.
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.write(true).custom_flags(0x0200_0000);
        }
        options.open(&path).unwrap().set_modified(time).unwrap();
        path
    }

    #[test]
    fn cleanup_enforces_age_and_total_but_keeps_the_current_item() {
        let dir = tempfile::tempdir().unwrap();
        let hour = Duration::from_secs(3600);
        let old = item(dir.path(), "old", 10, 30 * hour);
        let current_old = item(dir.path(), "current", 10, 30 * hour);
        let a = item(dir.path(), "a", 40, 3 * hour);
        let b = item(dir.path(), "b", 40, 2 * hour);
        let c = item(dir.path(), "c", 40, hour);
        let temp = item(dir.path(), "stale.tmp", 5, hour);
        let left = cleanup_cache(dir.path(), 100, 24 * hour, Some(&current_old), None);
        assert!(!old.exists(), "expired item");
        assert!(!temp.exists(), "interrupted staging");
        assert!(current_old.exists(), "the clipboard item is kept");
        assert!(!a.exists(), "oldest item evicted for the cap");
        assert!(b.exists() && c.exists());
        assert_eq!(left, 90);
    }

    #[test]
    fn clear_keeps_only_the_active_transfer() {
        let dir = tempfile::tempdir().unwrap();
        let hour = Duration::from_secs(3600);
        let done = item(dir.path(), "done", 10, hour);
        let active = item(dir.path(), "busy.tmp", 7, Duration::ZERO);
        assert_eq!(clear_cache(dir.path(), Some(&active)), 7);
        assert!(!done.exists() && active.exists());
    }
}
