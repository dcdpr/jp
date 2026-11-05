//! What the history of the default branch says about who wrote each fragment
//! and highlight.
//!
//! A release credits a file to every commit that wrote or changed it while it
//! sat at the top level of the fragments directory, up to the commit that filed
//! it into a release's directory.
//! History is read along the first-parent line, so a change that reached the
//! branch through a merge is the merge commit's, whose message names the change
//! request.
//!
//! [`History::read`] reads it once, to ask about any file: the commit that
//! brought it to the top level, the change requests that wrote it, and for a
//! commit, the files it added and where they are now.
//! [`parse`] reads the log `git::log` writes, and [`credited`] follows each
//! file back through it, renames included.

use std::{collections::HashMap, num::NonZeroU64};

use camino::Utf8Path;

use crate::{directory::is_unreleased, forge::Forge, git, release::ReleaseError};

/// What each commit's header in the log starts with, followed by its hash, a
/// line break, and its message.
pub(crate) const HEADER: &str = "commit ";

/// The first-parent history of `HEAD` in the repository holding a fragments
/// directory, limited to the commits that changed the directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// Newest first, as git writes them.
    commits: Vec<Commit>,
}

/// A fragment or highlight a commit added at the top level of the fragments
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// Its path below the fragments directory when the commit added it, such as
    /// `retry-idle-turns.md` or `highlights/fan-out.md`.
    pub path: String,

    /// Its path at `HEAD`, followed through every rename since, such as
    /// `0.3.0/retry-idle-turns.md`; `None` once a later commit deleted it.
    pub now: Option<String>,
}

impl History {
    /// Read the history of the repository holding the fragments directory
    /// `dir`.
    ///
    /// # Errors
    ///
    /// Refuses a shallow clone, which lacks the history, and returns a git
    /// command that failed or a log that doesn't read as one.
    pub fn read(dir: &Utf8Path) -> Result<Self, ReleaseError> {
        if git::is_shallow(dir)? {
            return Err(ReleaseError::Shallow);
        }
        let commits = parse(&git::log(dir)?).map_err(ReleaseError::Log)?;

        Ok(Self { commits })
    }

    pub(crate) fn commits(&self) -> &[Commit] {
        &self.commits
    }

    /// For each of `paths`, below the fragments directory at `HEAD`, the hash
    /// of the commit that brought it to the top level: the oldest commit that
    /// wrote or changed it there.
    ///
    /// `None` for a file no commit wrote at the top level, such as one added
    /// straight into a release's directory.
    #[must_use]
    pub fn origins(&self, paths: &[&str]) -> Vec<Option<String>> {
        credited(&self.commits, paths)
            .into_iter()
            .map(|indices| {
                let oldest = indices.first()?;
                Some(self.commits.get(*oldest)?.hash.clone())
            })
            .collect()
    }

    /// The fragments and highlights the commit `hash` added at the top level,
    /// each with where it is now.
    ///
    /// Empty when the commit added none, which includes a commit off the
    /// first-parent line of `HEAD`: its changes are the merge commit's.
    #[must_use]
    pub fn added_by(&self, hash: &str) -> Vec<Added> {
        let Some(index) = self.commits.iter().position(|commit| commit.hash == hash) else {
            return vec![];
        };

        self.commits[index]
            .changes
            .iter()
            .filter_map(|change| match change {
                Change::Added(path) if is_unreleased(path) => Some(Added {
                    path: path.clone(),
                    now: self.follow(index, path),
                }),
                _ => None,
            })
            .collect()
    }

    /// The change requests `forge` reads from the commits that wrote or changed
    /// `path`, below the fragments directory at `HEAD`, while it sat at the top
    /// level, oldest first and each once.
    #[must_use]
    pub fn change_requests(&self, path: &str, forge: Forge) -> Vec<NonZeroU64> {
        let indices = credited(&self.commits, &[path]).pop().unwrap_or_default();

        let mut numbers = vec![];
        for index in indices {
            let number = self
                .commits
                .get(index)
                .and_then(|commit| forge.change_request(&commit.message));
            if let Some(number) = number
                && !numbers.contains(&number)
            {
                numbers.push(number);
            }
        }

        numbers
    }

    /// Where the file at `path` after the commit at `index` is at `HEAD`:
    /// followed through every later rename, and `None` once a later commit
    /// deleted it.
    fn follow(&self, index: usize, path: &str) -> Option<String> {
        let mut path = path.to_owned();

        // The commits newer than the one at `index`, oldest of them first.
        for commit in self.commits[..index].iter().rev() {
            for change in &commit.changes {
                match change {
                    Change::Renamed { from, to } if *from == path => {
                        path.clone_from(to);
                        break;
                    }
                    Change::Deleted(deleted) if *deleted == path => return None,
                    _ => {}
                }
            }
        }

        Some(path)
    }
}

/// A commit on the first-parent line that changed the fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit {
    pub hash: String,

    /// The full message, subject and body.
    pub message: String,

    /// What the commit changed below the fragments directory, against its first
    /// parent.
    pub changes: Vec<Change>,
}

/// How a commit changed one path, below the fragments directory with `/`
/// between its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    Added(String),
    Modified(String),
    Deleted(String),
    Renamed { from: String, to: String },
}

/// Read the log `git::log` writes: each commit's header, `commit <hash>`, a
/// line break and its message, then each path it changed as `--name-status -z`
/// writes them, every field ended by a NUL.
///
/// The error names what in the log doesn't read as such.
pub(crate) fn parse(log: &str) -> Result<Vec<Commit>, String> {
    let mut commits: Vec<Commit> = vec![];
    let mut fields = log.split('\0');

    while let Some(field) = fields.next() {
        // A line break separates a commit's header from what it changed.
        let field = field.trim_start_matches('\n');
        if field.is_empty() {
            continue;
        }

        if let Some(header) = field.strip_prefix(HEADER) {
            let (hash, message) = header.split_once('\n').unwrap_or((header, ""));
            commits.push(Commit {
                hash: hash.to_owned(),
                message: message.to_owned(),
                changes: vec![],
            });
            continue;
        }

        let commit = commits
            .last_mut()
            .ok_or_else(|| format!("the log lists `{field}` before any commit"))?;
        let mut path = || {
            fields
                .next()
                .filter(|path| !path.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("the log names no path after `{field}`"))
        };

        let change = match status(field) {
            Some('A') => Change::Added(path()?),
            Some('M' | 'T') => Change::Modified(path()?),
            Some('D') => Change::Deleted(path()?),
            Some('R') => Change::Renamed {
                from: path()?,
                to: path()?,
            },
            // A copy leaves its source where it was: for the copy, history
            // starts here.
            Some('C') => {
                path()?;
                Change::Added(path()?)
            }
            _ => return Err(format!("the log lists an unknown change, `{field}`")),
        };
        commit.changes.push(change);
    }

    Ok(commits)
}

/// The letter of a `--name-status` status, such as `R` for `R100`: one
/// uppercase letter, and a similarity score for a rename or a copy.
fn status(field: &str) -> Option<char> {
    let mut chars = field.chars();
    let letter = chars.next().filter(char::is_ascii_uppercase)?;

    chars.all(|c| c.is_ascii_digit()).then_some(letter)
}

/// For each of `paths`, the indices into `commits` of the commits that wrote or
/// changed it while it sat at the top level of the fragments directory, oldest
/// first.
///
/// `commits` is the log newest first, as git writes it, and each of `paths` is
/// a file's path below the fragments directory now.
/// A file is followed back through every rename.
/// A rename counts only when it moves a file within the top level, so the
/// commit that files it into a release's directory counts for nothing, and
/// neither does any change to it there.
pub(crate) fn credited(commits: &[Commit], paths: &[&str]) -> Vec<Vec<usize>> {
    let mut followed: HashMap<String, usize> = paths
        .iter()
        .enumerate()
        .map(|(file, path)| ((*path).to_owned(), file))
        .collect();
    let mut credited = vec![vec![]; paths.len()];

    for (index, commit) in commits.iter().enumerate() {
        // Every change in a commit is read against where the files were after
        // it, so a path one change frees and another takes, in the same commit,
        // is followed to the right file.
        let mut ended = vec![];
        let mut moved = vec![];

        for change in &commit.changes {
            match change {
                Change::Added(path) | Change::Modified(path) => {
                    let Some(&file) = followed.get(path) else {
                        continue;
                    };
                    if is_unreleased(path) {
                        credited[file].push(index);
                    }
                    if matches!(change, Change::Added(_)) {
                        ended.push(path);
                    }
                }
                Change::Renamed { from, to } => {
                    let Some(&file) = followed.get(to) else {
                        continue;
                    };
                    if is_unreleased(from) && is_unreleased(to) {
                        credited[file].push(index);
                    }
                    ended.push(to);
                    moved.push((from, file));
                }
                // Nothing followed can have been deleted after the commit: the
                // commit that wrote it again ended its history.
                Change::Deleted(_) => {}
            }
        }

        for path in ended {
            followed.remove(path);
        }
        for (path, file) in moved {
            followed.insert(path.clone(), file);
        }
    }

    for commits in &mut credited {
        commits.reverse();
    }

    credited
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
