//! Per-skill filesystem transactions. A durable journal restores the previous
//! generation after interruption; changed files are never silently discarded.
use crate::files;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use toolbox_core::skills::SkillApp;

pub(crate) struct Change {
    pub destination: PathBuf,
    pub source: Option<PathBuf>,
    /// Existing content must match this digest; None requires an absent path.
    pub expected: Option<String>,
    pub replacement: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    destination: PathBuf,
    stage: Option<PathBuf>,
    backup: PathBuf,
    old: Option<String>,
    new: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    generation: u64,
    entries: Vec<Entry>,
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing state directory")?;
    if files::exists(path)? && !files::metadata(path)?.is_file() {
        return Err("Skills state is not a regular file".into());
    }
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist(path)
        .map_err(|e| format!("Cannot save skills state: {}", e.error))?;
    sync_dir(parent);
    Ok(())
}

fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Ok(directory) = fs::File::open(path) {
        let _ = directory.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn digest(path: &Path) -> Result<Option<String>, String> {
    if files::exists(path)? {
        files::fingerprint(path).map(Some)
    } else {
        Ok(None)
    }
}

fn same(path: &Path, expected: &Option<String>) -> Result<(), String> {
    if digest(path)? != *expected {
        return Err(format!(
            "Local content changed; preserve or move it before retrying: {}",
            path.display()
        ));
    }
    Ok(())
}

/// Parents must be canonical trusted roots or checked children of those roots.
pub(crate) fn execute(
    root: &Path,
    allowed_parents: &[PathBuf],
    generation: u64,
    next_state: &[u8],
    changes: Vec<Change>,
) -> Result<Option<String>, String> {
    if files::exists(&root.join("pending.json"))? {
        return Err("A previous skills transaction needs recovery; restart the host".into());
    }
    if changes.is_empty() {
        atomic_write(&root.join("state.json"), next_state)?;
        return Ok(None);
    }
    let mut entries = Vec::new();
    let mut stages = Vec::new();
    for change in changes {
        validate_destination(&change.destination, allowed_parents)?;
        let parent = change
            .destination
            .parent()
            .ok_or("Missing destination directory")?;
        let old = digest(&change.destination)?;
        if old.is_some() && old != change.expected {
            return Err(format!(
                "An unmanaged or locally edited skill already exists: {}",
                change.destination.display()
            ));
        }
        let (stage, new) = if let Some(source) = &change.source {
            let temporary = tempfile::Builder::new()
                .prefix(".sean-toolbox-stage-")
                .tempdir_in(parent)
                .map_err(|e| e.to_string())?;
            files::copy_tree(source, temporary.path())?;
            let hash = files::fingerprint(temporary.path())?;
            if Some(&hash) != change.replacement.as_ref() {
                return Err("Skill changed while staging; no installed copy was replaced".into());
            }
            let path = temporary.path().to_owned();
            stages.push(temporary);
            (Some(path), Some(hash))
        } else {
            (None, None)
        };
        let backup = tempfile::Builder::new()
            .prefix(".sean-toolbox-backup-")
            .tempdir_in(parent)
            .map_err(|e| e.to_string())?;
        let backup_path = backup.path().to_owned();
        backup.close().map_err(|e| e.to_string())?;
        entries.push(Entry {
            destination: change.destination,
            stage,
            backup: backup_path,
            old,
            new,
        });
    }
    let journal = Journal {
        generation,
        entries,
    };
    let bytes = serde_json::to_vec_pretty(&journal).map_err(|e| e.to_string())?;
    atomic_write(&root.join("pending.json"), &bytes)?;
    // The journal now owns these paths, including across process interruption.
    for stage in stages {
        let _ = stage.keep();
    }
    let apply = (|| {
        for entry in &journal.entries {
            same(&entry.destination, &entry.old)?;
            if entry.old.is_some() {
                fs::rename(&entry.destination, &entry.backup)
                    .map_err(|e| format!("Cannot move skill aside: {e}"))?;
            }
            if let Some(stage) = &entry.stage {
                fs::rename(stage, &entry.destination)
                    .map_err(|e| format!("Cannot install skill: {e}"))?;
            }
            sync_dir(
                entry
                    .destination
                    .parent()
                    .ok_or("Missing destination parent")?,
            );
        }
        atomic_write(&root.join("state.json"), next_state)
    })();
    if let Err(error) = apply {
        let warnings = rollback(&journal).map_err(|recovery| format!("{error}; rollback needs attention: {recovery}. Previous copies and pending.json were preserved"))?;
        fs::remove_file(root.join("pending.json")).map_err(|e| e.to_string())?;
        return Err(if warnings.is_empty() {
            error
        } else {
            format!("{error}; {}", warnings.join("; "))
        });
    }
    let mut warnings = cleanup(&journal);
    if let Err(error) = fs::remove_file(root.join("pending.json")) {
        warnings.push(format!(
            "Saved; transaction journal cleanup will retry after restart: {error}"
        ));
    }
    Ok((!warnings.is_empty()).then(|| warnings.join("; ")))
}

fn validate_destination(path: &Path, allowed_parents: &[PathBuf]) -> Result<(), String> {
    if !path
        .parent()
        .is_some_and(|parent| allowed_parents.iter().any(|allowed| allowed == parent))
    {
        return Err("Transaction destination escapes the skills directories".into());
    }
    let parent = path.parent().ok_or("Missing transaction parent")?;
    if parent
        .canonicalize()
        .map_err(|e| format!("Cannot verify transaction directory: {e}"))?
        != parent
    {
        return Err("Transaction directory contains a symlink or redirected path".into());
    }
    if !files::metadata(parent)?.is_dir() {
        return Err("Transaction directory is not a directory".into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid skill directory")?;
    files::name(name)?;
    if name.starts_with(".sean-toolbox-") {
        return Err("Reserved skills transaction directory".into());
    }
    Ok(())
}

fn validate_auxiliary(path: &Path, entry: &Entry, prefix: &str) -> Result<(), String> {
    if path.parent() != entry.destination.parent()
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix) && files::name(name).is_ok())
    {
        return Err("Invalid skills recovery path".into());
    }
    Ok(())
}

pub(crate) fn recover(
    root: &Path,
    allowed_parents: &[PathBuf],
    generation: u64,
) -> Result<Vec<String>, String> {
    let path = root.join("pending.json");
    if !files::exists(&path)? {
        return Ok(Vec::new());
    }
    let journal: Journal = serde_json::from_slice(&files::read_limited(&path, 1024 * 1024)?)
        .map_err(|e| format!("Invalid recovery journal: {e}"))?;
    if journal.entries.len() > 1 + SkillApp::ALL.len() || journal.entries.is_empty() {
        return Err("Invalid skills transaction entry count".into());
    }
    let mut present = Journal {
        generation: journal.generation,
        entries: Vec::new(),
    };
    let mut warnings = Vec::new();
    for entry in journal.entries {
        let parent = entry
            .destination
            .parent()
            .ok_or("Missing destination parent")?;
        // A directory removed after the interruption cannot hold a recoverable
        // copy. Skip that entry instead of blocking every future command.
        if !parent.exists() {
            warnings.push(format!(
                "Skipped a skills recovery entry because its directory is missing: {}",
                parent.display()
            ));
            continue;
        }
        validate_destination(&entry.destination, allowed_parents)?;
        validate_auxiliary(&entry.backup, &entry, ".sean-toolbox-backup-")?;
        if let Some(stage) = &entry.stage {
            validate_auxiliary(stage, &entry, ".sean-toolbox-stage-")?;
        }
        present.entries.push(entry);
    }
    if journal.generation != generation && journal.generation != generation.saturating_add(1) {
        return Err(
            "Skills transaction generation does not match state; recovery files were preserved"
                .into(),
        );
    }
    if !present.entries.is_empty() {
        let outcome = if journal.generation == generation {
            cleanup(&present)
        } else {
            rollback(&present)?
        };
        warnings.extend(outcome);
    }
    fs::remove_file(path).map_err(|e| e.to_string())?;
    Ok(warnings)
}

fn remove_expected(path: &Path, expected: &Option<String>) -> Result<(), String> {
    if files::exists(path)? {
        same(path, expected)?;
        fs::remove_dir_all(path).map_err(|e| format!("Cannot clean up {}: {e}", path.display()))?;
    }
    Ok(())
}

fn rollback(journal: &Journal) -> Result<Vec<String>, String> {
    for entry in journal.entries.iter().rev() {
        if files::exists(&entry.backup)? {
            same(&entry.backup, &entry.old)?;
            move_new_to_stage(entry)?;
            fs::rename(&entry.backup, &entry.destination).map_err(|e| e.to_string())?;
        } else if entry.old.is_none() {
            move_new_to_stage(entry)?;
        } else {
            same(&entry.destination, &entry.old)?;
        }
        sync_dir(
            entry
                .destination
                .parent()
                .ok_or("Missing destination parent")?,
        );
    }
    // Restoration above uses only renames. A partially deleted temporary copy
    // must never prevent future startup after the original is back in place.
    Ok(cleanup(journal))
}

fn move_new_to_stage(entry: &Entry) -> Result<(), String> {
    if !files::exists(&entry.destination)? {
        return Ok(());
    }
    same(&entry.destination, &entry.new)?;
    let stage = entry
        .stage
        .as_ref()
        .ok_or("Unexpected destination while restoring a removal")?;
    if files::exists(stage)? {
        return Err(
            "Both staged and installed copies exist during recovery; preserved for inspection"
                .into(),
        );
    }
    fs::rename(&entry.destination, stage).map_err(|e| e.to_string())
}

fn cleanup(journal: &Journal) -> Vec<String> {
    let mut warnings = Vec::new();
    for entry in &journal.entries {
        for (path, expected) in std::iter::once((&entry.backup, &entry.old))
            .chain(entry.stage.as_ref().map(|stage| (stage, &entry.new)))
        {
            if let Err(error) = remove_expected(path, expected) {
                warnings.push(format!(
                    "Preserved leftover temporary copy {}: {error}",
                    path.display()
                ));
            }
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(path: &Path, value: &str) -> String {
        fs::create_dir(path).unwrap();
        fs::write(path.join("SKILL.md"), value).unwrap();
        fs::write(path.join("data.txt"), format!("data {value}")).unwrap();
        files::fingerprint(path).unwrap()
    }

    fn journal(root: &Path, generation: u64, entry: Entry) {
        atomic_write(
            &root.join("pending.json"),
            &serde_json::to_vec(&Journal {
                generation,
                entries: vec![entry],
            })
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn interrupted_replacement_restores_original_before_exposing_service() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let destination = root.join("review");
        let stage = root.join(".sean-toolbox-stage-fixture");
        let backup = root.join(".sean-toolbox-backup-fixture");
        let old = tree(&destination, "old");
        let new = tree(&stage, "new");
        fs::rename(&destination, &backup).unwrap();
        fs::rename(&stage, &destination).unwrap();
        journal(
            &root,
            2,
            Entry {
                destination: destination.clone(),
                stage: Some(stage.clone()),
                backup: backup.clone(),
                old: Some(old),
                new: Some(new),
            },
        );
        assert!(
            recover(&root, std::slice::from_ref(&root), 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            "old"
        );
        assert!(!backup.exists());
        assert!(!stage.exists());
        assert!(!root.join("pending.json").exists());
    }

    #[test]
    fn partial_backup_cleanup_after_commit_preserves_leftover_without_blocking_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let destination = root.join("review");
        let backup = root.join(".sean-toolbox-backup-fixture");
        let new = tree(&destination, "new");
        let old = tree(&backup, "old");
        // This is the on-disk state after Windows deleted one file, then hit a
        // sharing violation on the next file, or the process was interrupted.
        fs::remove_file(backup.join("data.txt")).unwrap();
        journal(
            &root,
            2,
            Entry {
                destination: destination.clone(),
                stage: Some(root.join(".sean-toolbox-stage-fixture")),
                backup: backup.clone(),
                old: Some(old),
                new: Some(new),
            },
        );
        let warnings = recover(&root, std::slice::from_ref(&root), 2).unwrap();
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            "new"
        );
        assert_eq!(fs::read_to_string(backup.join("SKILL.md")).unwrap(), "old");
        assert!(!root.join("pending.json").exists());
        assert!(
            recover(&root, std::slice::from_ref(&root), 2)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn rollback_with_partially_cleaned_stage_keeps_original_and_retires_journal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let destination = root.join("review");
        let stage = root.join(".sean-toolbox-stage-fixture");
        let old = tree(&destination, "old restored");
        let new = tree(&stage, "discarded new");
        fs::remove_file(stage.join("data.txt")).unwrap();
        journal(
            &root,
            2,
            Entry {
                destination: destination.clone(),
                stage: Some(stage.clone()),
                backup: root.join(".sean-toolbox-backup-fixture"),
                old: Some(old),
                new: Some(new),
            },
        );
        assert_eq!(
            recover(&root, std::slice::from_ref(&root), 1)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            "old restored"
        );
        assert!(stage.exists());
        assert!(!root.join("pending.json").exists());
    }

    #[test]
    fn failed_state_write_rolls_back_every_replaced_copy() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let first = root.join("first");
        let second = root.join("second");
        let new = tree(&source, "new");
        let old = tree(&first, "old");
        tree(&second, "old");
        fs::create_dir(root.join("state.json")).unwrap();
        let changes = [first.clone(), second.clone()]
            .into_iter()
            .map(|destination| Change {
                destination,
                source: Some(source.clone()),
                expected: Some(old.clone()),
                replacement: Some(new.clone()),
            })
            .collect();
        assert!(execute(&root, std::slice::from_ref(&root), 2, b"{}", changes).is_err());
        for path in [first, second] {
            assert_eq!(fs::read_to_string(path.join("SKILL.md")).unwrap(), "old");
        }
        assert!(!root.join("pending.json").exists());
    }

    #[test]
    fn recovery_refuses_changed_live_destination_and_preserves_both_copies() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let destination = root.join("review");
        let backup = root.join(".sean-toolbox-backup-fixture");
        let old = tree(&backup, "old");
        let new = tree(&destination, "new");
        fs::write(
            destination.join("SKILL.md"),
            "user changed after interruption",
        )
        .unwrap();
        journal(
            &root,
            2,
            Entry {
                destination: destination.clone(),
                stage: Some(root.join(".sean-toolbox-stage-fixture")),
                backup: backup.clone(),
                old: Some(old),
                new: Some(new),
            },
        );
        assert!(recover(&root, std::slice::from_ref(&root), 1).is_err());
        assert_eq!(
            fs::read_to_string(destination.join("SKILL.md")).unwrap(),
            "user changed after interruption"
        );
        assert_eq!(fs::read_to_string(backup.join("SKILL.md")).unwrap(), "old");
        assert!(root.join("pending.json").exists());
        fs::write(root.join("state.json"), b"old generation").unwrap();
        // An unrelated, unchanged skill must not advance the generation and
        // make this unresolved replacement appear committed after a restart.
        assert!(
            execute(
                &root,
                std::slice::from_ref(&root),
                2,
                b"new generation",
                Vec::new()
            )
            .is_err()
        );
        assert_eq!(
            fs::read(root.join("state.json")).unwrap(),
            b"old generation"
        );
        assert_eq!(fs::read_to_string(backup.join("SKILL.md")).unwrap(), "old");
    }

    #[test]
    fn recovery_retires_entries_whose_app_directory_was_removed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let app = root.join("app");
        fs::create_dir(&app).unwrap();
        let destination = app.join("review");
        let stage = app.join(".sean-toolbox-stage-fixture");
        let backup = app.join(".sean-toolbox-backup-fixture");
        let old = tree(&destination, "old");
        let new = tree(&stage, "new");
        fs::rename(&destination, &backup).unwrap();
        fs::rename(&stage, &destination).unwrap();
        journal(
            &root,
            2,
            Entry {
                destination: destination.clone(),
                stage: Some(stage.clone()),
                backup: backup.clone(),
                old: Some(old),
                new: Some(new),
            },
        );
        fs::remove_dir_all(&app).unwrap();
        let warnings = recover(&root, std::slice::from_ref(&app), 1).unwrap();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("directory is missing")),
            "{warnings:?}"
        );
        assert!(!root.join("pending.json").exists());
    }
}
