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
//! [`parse`] reads the log `git::log` writes, and [`credited`] follows each
//! file back through it, renames included.

use std::collections::HashMap;

use crate::directory::is_unreleased;

/// What each commit's header in the log starts with, followed by its hash, a
/// line break, and its message.
pub(crate) const HEADER: &str = "commit ";

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
