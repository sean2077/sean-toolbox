use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

pub const MAX_SKILL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FILES: usize = 4096;
const MAX_DEPTH: usize = 20;
pub const MAX_PATH: usize = 512;

pub fn limited(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

pub fn name(value: &str) -> Result<(), String> {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let device = matches!(stem.as_str(), "con" | "prn" | "aux" | "nul" | "clock$")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.as_bytes()[3].is_ascii_digit());
    if value.is_empty()
        || value.len() > 128
        || value == "."
        || value == ".."
        || value.ends_with(['.', ' '])
        || value
            .chars()
            .any(|c| c.is_control() || "<>:\"/\\|?*".contains(c))
        || device
    {
        return Err(format!(
            "Unsafe or non-portable file name: {}",
            limited(value, 128)
        ));
    }
    Ok(())
}

pub fn relative(value: &str) -> Result<PathBuf, String> {
    if value.len() > MAX_PATH {
        return Err("Skill path is too long".into());
    }
    let mut path = PathBuf::new();
    if value.is_empty() {
        return Ok(path);
    }
    for part in value.split('/') {
        name(part)?;
        if part.eq_ignore_ascii_case(".git") {
            return Err("A skill path cannot enter .git".into());
        }
        path.push(part);
    }
    Ok(path)
}

pub fn metadata(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut link = metadata.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        link |= metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    let _ = &mut link;
    if link {
        return Err(format!(
            "Symlinks and reparse points are not managed: {}",
            path.display()
        ));
    }
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(format!(
            "Only regular files and directories are supported: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

/// Check every component below an already canonical, trusted root.
pub fn checked_path(root: &Path, path: &Path, create: bool) -> Result<(), String> {
    let suffix = path
        .strip_prefix(root)
        .map_err(|_| "Path escapes its managed root")?;
    if !metadata(root)?.is_dir() {
        return Err("Managed root is not a directory".into());
    }
    let mut current = root.to_path_buf();
    let components: Vec<_> = suffix.components().collect();
    for component in components.iter() {
        let Component::Normal(part) = component else {
            return Err("Only normal path components are allowed".into());
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(_) => {
                let meta = metadata(&current)?;
                if !meta.is_dir() {
                    return Err(format!("Not a directory: {}", current.display()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if create {
                    fs::create_dir(&current).map_err(|e| format!("{}: {e}", current.display()))?;
                    metadata(&current)?;
                }
            }
            Err(error) => return Err(format!("{}: {error}", current.display())),
        }
    }
    Ok(())
}

pub fn exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            metadata(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Return the raw link target when `path` is a symlink or reparse point.
pub fn link_target(path: &Path) -> Result<Option<PathBuf>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut link = metadata.file_type().is_symlink();
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        link |= metadata.file_attributes() & 0x400 != 0;
    }
    #[cfg(not(windows))]
    let _ = &mut link;
    if link {
        fs::read_link(path)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display()))
    } else {
        Ok(None)
    }
}

/// Create a directory symlink. Windows requires Developer Mode or the
/// SeCreateSymbolicLinkPrivilege.
pub fn link_dir(target: &Path, link: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let result = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let result = std::os::windows::fs::symlink_dir(target, link);
    result.map_err(|error| {
        format!(
            "Cannot create link {} -> {}: {error} (on Windows, enable Developer Mode or grant symlink permission)",
            link.display(),
            target.display()
        )
    })
}

pub fn remove_link(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    let result = fs::remove_file(path);
    #[cfg(windows)]
    let result = fs::remove_dir(path);
    result.map_err(|e| format!("Cannot remove link {}: {e}", path.display()))
}

/// True when `path` is a link that resolves to `target`.
pub fn links_to(path: &Path, target: &Path) -> Result<bool, String> {
    if link_target(path)?.is_none() {
        return Ok(false);
    }
    let actual = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let target = target
        .canonicalize()
        .map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(actual == target)
}

pub fn sorted_entries(path: &Path) -> Result<Vec<fs::DirEntry>, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

#[derive(Default)]
struct Budget {
    files: usize,
    bytes: u64,
}

impl Budget {
    fn add(&mut self, size: u64, depth: usize) -> Result<(), String> {
        self.files += 1;
        self.bytes = self.bytes.saturating_add(size);
        if self.files > MAX_FILES
            || self.bytes > MAX_SKILL_BYTES
            || size > MAX_FILE_BYTES
            || depth > MAX_DEPTH
        {
            return Err(
                "Skill exceeds limits (32 MiB total, 8 MiB per file, 4096 entries, 20 levels)"
                    .into(),
            );
        }
        Ok(())
    }
}

pub fn fingerprint(path: &Path) -> Result<String, String> {
    if !metadata(path)?.is_dir() {
        return Err(format!("Skill is not a directory: {}", path.display()));
    }
    let mut hash = Sha256::new();
    hash_tree(path, Path::new(""), &mut hash, &mut Budget::default(), 0)?;
    Ok(format!("{:x}", hash.finalize()))
}

fn hash_tree(
    root: &Path,
    relative: &Path,
    hash: &mut Sha256,
    budget: &mut Budget,
    depth: usize,
) -> Result<(), String> {
    let path = root.join(relative);
    let meta = metadata(&path)?;
    let text = relative
        .to_str()
        .ok_or("Non-Unicode skill path")?
        .replace('\\', "/");
    if text.len() > MAX_PATH {
        return Err("Skill path is too long".into());
    }
    hash.update((text.len() as u64).to_le_bytes());
    hash.update(text.as_bytes());
    // The package root is not one of the entries users count against the
    // documented 4096-entry budget; copy_inner also counts children only.
    if depth > 0 {
        budget.add(if meta.is_file() { meta.len() } else { 0 }, depth)?;
    }
    if meta.is_dir() {
        hash.update([0]);
        let mut names = HashSet::new();
        for entry in sorted_entries(&path)? {
            let file_name = entry.file_name();
            let file_name = file_name.to_str().ok_or("Non-Unicode skill file name")?;
            name(file_name)?;
            if !names.insert(file_name.to_lowercase()) {
                return Err("Skill contains file names that collide on Windows".into());
            }
            hash_tree(root, &relative.join(file_name), hash, budget, depth + 1)?;
        }
    } else {
        hash.update([1]);
        hash.update(meta.len().to_le_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            hash.update([u8::from(meta.permissions().mode() & 0o111 != 0)]);
        }
        let mut file = fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut buffer = [0; 16 * 1024];
        let mut length = 0;
        loop {
            let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            length += read as u64;
            if length > MAX_FILE_BYTES {
                return Err("Skill file grew beyond its size limit".into());
            }
            hash.update(&buffer[..read]);
        }
        if length != meta.len() {
            return Err("Skill changed while being read; try again".into());
        }
    }
    Ok(())
}

pub fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    copy_inner(source, target, &mut Budget::default(), 0)?;
    // Validate portable paths, collisions and all copied bytes before deployment.
    fingerprint(target)?;
    Ok(())
}

fn copy_inner(
    source: &Path,
    target: &Path,
    budget: &mut Budget,
    depth: usize,
) -> Result<(), String> {
    if !metadata(source)?.is_dir() {
        return Err("Skill source is not a directory".into());
    }
    for entry in sorted_entries(source)? {
        let file_name = entry.file_name();
        let text = file_name.to_str().ok_or("Non-Unicode skill file name")?;
        name(text)?;
        // A local repository may itself be a skill; never copy its Git database.
        if text.eq_ignore_ascii_case(".git") {
            continue;
        }
        let path = entry.path();
        let meta = metadata(&path)?;
        budget.add(if meta.is_file() { meta.len() } else { 0 }, depth + 1)?;
        let destination = target.join(&file_name);
        if meta.is_dir() {
            fs::create_dir(&destination).map_err(|e| e.to_string())?;
            copy_inner(&path, &destination, budget, depth + 1)?;
        } else {
            let mut input = fs::File::open(&path).map_err(|e| e.to_string())?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|e| e.to_string())?;
            let length = std::io::copy(&mut (&mut input).take(MAX_FILE_BYTES + 1), &mut output)
                .map_err(|e| e.to_string())?;
            if length != meta.len() || length > MAX_FILE_BYTES {
                return Err("Skill changed while being copied; try again".into());
            }
            output
                .flush()
                .and_then(|()| output.sync_all())
                .map_err(|e| e.to_string())?;
            fs::set_permissions(&destination, meta.permissions()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn read_limited(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    if !metadata(path)?.is_file() {
        return Err(format!("Not a regular file: {}", path.display()));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max {
        return Err(format!("File exceeds {max} bytes: {}", path.display()));
    }
    Ok(bytes)
}

pub fn descriptor(directory: &Path, fallback: &str) -> Result<(String, String), String> {
    let bytes = read_limited(&directory.join("SKILL.md"), 64 * 1024)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "SKILL.md must be UTF-8")?;
    let text = text.trim_start_matches('\u{feff}');
    if text.trim().is_empty() {
        return Err("SKILL.md is empty".into());
    }
    let mut title = fallback.to_owned();
    let mut description = String::new();
    let mut in_frontmatter = false;
    let mut block_description = false;
    for (index, line) in text.lines().enumerate() {
        // Frontmatter delimiters start at column 0. An indented `---` inside a
        // block scalar is content, not the end of the header.
        if index == 0 && line.trim_end() == "---" {
            in_frontmatter = true;
            continue;
        }
        if !in_frontmatter || line.trim_end() == "---" {
            break;
        }
        if block_description && (line.starts_with(' ') || line.starts_with('\t')) {
            if !description.is_empty() {
                description.push(' ');
            }
            description.push_str(line.trim());
            continue;
        }
        block_description = false;
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim().trim_matches(['\'', '"']);
            if !value.is_empty() {
                title = value.into();
            }
        }
        if let Some(value) = line.strip_prefix("description:") {
            let value = value.trim().trim_matches(['\'', '"']);
            if matches!(value, ">" | ">-" | "|" | "|-") {
                block_description = true;
            } else {
                description = value.into();
            }
        }
    }
    Ok((limited(&title, 80), limited(&description, 240)))
}
