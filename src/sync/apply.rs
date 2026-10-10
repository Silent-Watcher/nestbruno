//! Carries out a [`Plan`] on the filesystem.
//!
//! Only `Create`, `Update` and `Delete` entries do anything. Every operation
//! goes through the safe primitives of [`crate::bruno::writer`]: containment,
//! no symlinks, atomic replacement, and, for `Update` and `Delete`, a re-check
//! that the file still has exactly the content that was classified. A file that
//! changed in the meantime is left alone and reported in
//! [`SyncResult::failed`]. A failure of one operation does not stop the others.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{Action, Plan, SyncResult};
use crate::bruno::writer::{
    COLLECTION_FILE, COLLECTION_JSON, FileStatus, check_component, ensure_dir, existing_dir,
    relative_components, remove_empty_dirs, remove_if_unchanged, replace_if_unchanged, write_file,
};
use crate::error::Error;

/// Apply `plan` below `root/bruno_dir`, recording what happened in `result`.
pub fn apply(
    root: &Path,
    bruno_dir: &Path,
    plan: &Plan,
    result: &mut SyncResult,
) -> Result<(), Error> {
    result.applied = true;
    let operations = plan
        .entries
        .iter()
        .filter(|e| matches!(e.action, Action::Create | Action::Update | Action::Delete))
        .count();
    if operations == 0 {
        return Ok(());
    }

    let unsafe_output = |reason: String| Error::UnsafeOutput {
        path: bruno_dir.to_owned(),
        reason,
    };
    let components = relative_components(bruno_dir).ok_or_else(|| {
        unsafe_output("the collection directory must be a relative path without '..'".to_owned())
    })?;
    let creating = plan.entries.iter().any(|e| e.action == Action::Create);
    let collection = if creating {
        ensure_dir(root, &components)
    } else {
        existing_dir(root, &components)
    }
    .map_err(unsafe_output)?;
    if creating {
        // An existing bruno.json is the user's and is never replaced.
        if let Err(reason) = write_file(&collection, COLLECTION_FILE, COLLECTION_JSON, false, true)
        {
            result.failed.push((PathBuf::from(COLLECTION_FILE), reason));
        }
    }

    // Report what actually happened, not what was planned.
    result.created.clear();
    result.updated.clear();
    result.deleted.clear();

    let mut directories: HashMap<PathBuf, Result<PathBuf, String>> = HashMap::new();
    let mut emptied: Vec<PathBuf> = Vec::new();
    for entry in &plan.entries {
        if !matches!(
            entry.action,
            Action::Create | Action::Update | Action::Delete
        ) {
            continue;
        }
        let outcome = (|| {
            let (parent, name) = split(&entry.path)?;
            let dir = directories
                .entry(parent.clone())
                .or_insert_with(|| {
                    let parts = folder_components(&parent)?;
                    if entry.action == Action::Create {
                        ensure_dir(&collection, &parts)
                    } else {
                        existing_dir(&collection, &parts)
                    }
                })
                .clone()?;
            let content = entry.content.as_deref().unwrap_or_default();
            let expected = entry.expected.unwrap_or_default();
            match entry.action {
                Action::Create => match write_file(&dir, &name, content, false, false)? {
                    FileStatus::Created => Ok(()),
                    _ => Err(
                        "a file appeared at this path while synchronizing; left untouched"
                            .to_owned(),
                    ),
                },
                Action::Update => replace_if_unchanged(&dir, &name, content, expected),
                _ => {
                    remove_if_unchanged(&dir, &name, expected)?;
                    // Folders are pruned after all operations: a later entry of
                    // this run may still create a file in the same folder.
                    emptied.push(dir);
                    Ok(())
                }
            }
        })();
        match (outcome, entry.action) {
            (Ok(()), Action::Create) => result.created.push(entry.path.clone()),
            (Ok(()), Action::Update) => result.updated.push(entry.path.clone()),
            (Ok(()), _) => result.deleted.push(entry.path.clone()),
            (Err(reason), _) => result.failed.push((entry.path.clone(), reason)),
        }
    }
    // Remove folders left empty by deletions, deepest first. A folder that
    // still holds anything (including a file created above) is kept.
    emptied.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
    emptied.dedup();
    for dir in emptied {
        remove_empty_dirs(dir, &collection);
    }
    Ok(())
}

/// The components of a folder path relative to the collection. The empty path
/// (a file directly in the collection) has none; an unusable path is an error
/// and must never be mistaken for the collection root.
fn folder_components(folder: &Path) -> Result<Vec<String>, String> {
    if folder.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    relative_components(folder)
        .ok_or_else(|| format!("{} is not a safe folder path", folder.display()))
}

/// Split `Folder/file.bru` into its parent (relative) and file name, checking
/// every component.
fn split(path: &Path) -> Result<(PathBuf, String), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{} has no valid file name", path.display()))?;
    check_component(name)?;
    let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok((parent, name.to_owned()))
}
