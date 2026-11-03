//! What syncing changes in a project: the owned files that are missing or
//! differ from what the binary writes, the files under `.vitepress/jp-vite/`
//! the binary no longer has, and the starting pages the project doesn't have.
//!
//! [`plan`] and [`missing`] read the project and change nothing; [`apply`]
//! writes what they found.
//! Whether the project may be written at all is [`crate::git`]'s to say.

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write as _},
};

use camino::{Utf8Path, Utf8PathBuf};

use crate::site::{self, Owned};

/// One change syncing makes, to a path relative to the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    /// Write an owned file that is missing or differs.
    Write(Owned),

    /// Create a page the project doesn't have.
    Create(Owned),

    /// Remove a file under `.vitepress/jp-vite/` the binary doesn't have.
    Remove(Utf8PathBuf),
}

impl Change {
    /// The path the change touches.
    pub(crate) fn path(&self) -> &Utf8Path {
        match self {
            Self::Write(owned) | Self::Create(owned) => &owned.path,
            Self::Remove(path) => path,
        }
    }
}

/// The changes that bring the project at `root` in line with `owned`.
///
/// `owned_dir` is the directory the binary owns whole, relative to the root: a
/// file in it that `owned` doesn't name is removed.
/// Dotfiles are left alone, as what an editor or a file manager leaves behind.
pub(crate) fn plan(
    root: &Utf8Path,
    owned: Vec<Owned>,
    owned_dir: &Utf8Path,
) -> Result<Vec<Change>, String> {
    let named: BTreeSet<Utf8PathBuf> = owned.iter().map(|file| file.path.clone()).collect();
    let mut changes = vec![];

    for file in owned {
        let path = root.join(&file.path);
        match fs::read(&path) {
            Ok(contents) if same_text(&contents, &file.contents) => {}
            Ok(_) => changes.push(Change::Write(file)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                changes.push(Change::Write(file));
            }
            Err(error) => return Err(format!("{path}: {error}")),
        }
    }

    for path in files_below(root, owned_dir)? {
        if !named.contains(&path) {
            changes.push(Change::Remove(path));
        }
    }

    changes.sort_by(|a, b| a.path().cmp(b.path()));

    Ok(changes)
}

/// Whether a file holds the same text as `other`, however its lines end.
///
/// Git can check text files out with CRLF line endings (`core.autocrlf`, on by
/// default in Git for Windows) and commits them back with LF, so a file that
/// differs from what `jp vite` writes only there is, to git, the same file.
/// A `\r` that doesn't end a line still counts.
pub(crate) fn same_text(file: &[u8], other: &[u8]) -> bool {
    fn lf(bytes: &[u8]) -> impl Iterator<Item = u8> + '_ {
        bytes
            .iter()
            .enumerate()
            .filter(|&(index, &byte)| byte != b'\r' || bytes.get(index + 1) != Some(&b'\n'))
            .map(|(_, &byte)| byte)
    }

    lf(file).eq(lf(other))
}

/// The pages among `pages` that the project at `root` doesn't have.
///
/// A page that exists is the project's, whatever it holds.
pub(crate) fn missing(root: &Utf8Path, pages: Vec<Owned>) -> Vec<Change> {
    pages
        .into_iter()
        .filter(|page| !root.join(&page.path).exists())
        .map(Change::Create)
        .collect()
}

/// Make the changes, creating directories as needed.
///
/// A directory a removal empties is removed with it, up to `owned_dir`.
/// A page to create that exists by now, made while the changes waited to be
/// confirmed, is the project's and is left as it is; those are returned, as
/// paths relative to the root.
pub(crate) fn apply(
    root: &Utf8Path,
    changes: &[Change],
    owned_dir: &Utf8Path,
) -> Result<Vec<Utf8PathBuf>, String> {
    let mut kept = vec![];

    for change in changes {
        let path = root.join(change.path());
        match change {
            Change::Write(owned) => {
                create_parent(&path)?;
                fs::write(&path, &owned.contents).map_err(|error| format!("{path}: {error}"))?;
            }
            Change::Create(page) => {
                create_parent(&path)?;
                let created = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .and_then(|mut file| file.write_all(&page.contents));
                match created {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        kept.push(page.path.clone());
                    }
                    Err(error) => return Err(format!("{path}: {error}")),
                }
            }
            Change::Remove(_) => {
                fs::remove_file(&path).map_err(|error| format!("{path}: {error}"))?;
                remove_empty_parents(&path, &root.join(owned_dir));
            }
        }
    }

    Ok(kept)
}

fn create_parent(path: &Utf8Path) -> Result<(), String> {
    match path.parent() {
        Some(parent) => fs::create_dir_all(parent).map_err(|error| format!("{parent}: {error}")),
        None => Ok(()),
    }
}

/// Every file below `dir`, relative to `root`, dotfiles and the contents of
/// dot-directories left out.
/// A directory that doesn't exist holds none.
fn files_below(root: &Utf8Path, dir: &Utf8Path) -> Result<Vec<Utf8PathBuf>, String> {
    let mut found = vec![];
    let mut pending = vec![dir.to_path_buf()];

    while let Some(dir) = pending.pop() {
        let absolute = root.join(&dir);
        let entries = match fs::read_dir(&absolute) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("{absolute}: {error}")),
        };

        for entry in entries {
            let entry = entry.map_err(|error| format!("{absolute}: {error}"))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }

            let kind = entry
                .file_type()
                .map_err(|error| format!("{absolute}/{name}: {error}"))?;
            if kind.is_dir() {
                pending.push(site::join(&dir, &name));
            } else {
                found.push(site::join(&dir, &name));
            }
        }
    }

    Ok(found)
}

/// Remove the directories above `path` that are now empty, stopping at `top`.
///
/// Best effort: a directory that is still in use, or can't be removed, stays.
fn remove_empty_parents(path: &Utf8Path, top: &Utf8Path) {
    let mut dir = path.parent();
    while let Some(current) = dir {
        if current == top || !current.starts_with(top) || fs::remove_dir(current).is_err() {
            return;
        }
        dir = current.parent();
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
