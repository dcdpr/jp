//! First-parent git history tracking for fragments and highlights.
//!
//! Each file is credited to the commits that added, modified, or renamed it at
//! the top level of the fragments directory before it was moved into a
//! `<version>/` release subdirectory.
//! Because history is walked along the first-parent line of `HEAD`, changes
//! merged from a branch are attributed to the merge or squash commit on the
//! default branch.
//!
//! [`History::read`] loads the commit log once and answers queries about file
//! origins ([`History::origins`]), files added by a commit
//! ([`History::added_by`]), and credits ([`History::credits`]).

use std::collections::HashMap;

use camino::Utf8Path;

use crate::{
    directory::is_unreleased, document::ListedCredit, forge::Forge, git, release::ReleaseError,
};

/// Commit header prefix in `git::log` output, followed by `<hash>\n<message>`.
pub(crate) const HEADER: &str = "commit ";

/// First-parent commit history of `HEAD` touching a fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// Commits ordered from newest to oldest.
    commits: Vec<Commit>,
}

/// A fragment or highlight added at the top level of the fragments directory by
/// a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// Path relative to the fragments directory when the commit added it (e.g.
    /// `retry-idle-turns.fixed.md` or `highlights/fan-out.md`).
    pub path: String,

    /// Current path at `HEAD` after following subsequent renames (e.g.
    /// `0.3.0/retry-idle-turns.fixed.md`), or `None` if a later commit deleted
    /// the file.
    pub now: Option<String>,
}

impl History {
    /// Load the first-parent commit history touching `dir`.
    ///
    /// # Errors
    ///
    /// Returns [`ReleaseError::Shallow`] if the repository is a shallow clone,
    /// [`ReleaseError::Git`] if `git` fails, or [`ReleaseError::Log`] if the
    /// `git log` output cannot be parsed.
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

    /// Return the hash of the commit that first added each path in `paths` at
    /// the top level of the fragments directory.
    ///
    /// Returns `None` for any file that was never committed at the top level
    /// (for example, a file added directly inside a `<version>/` subdirectory).
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

    /// Return the top-level fragments and highlights added by commit `hash`,
    /// along with their current paths at `HEAD`.
    ///
    /// Returns an empty vector if `hash` is not on the first-parent history or
    /// added no top-level files.
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

    /// Return the deduplicated credits for `path` (relative to the fragments
    /// directory at `HEAD`), ordered from oldest to newest commit that touched
    /// the file at the top level.
    ///
    /// Each commit is credited as its change request on `forge`, or as its
    /// commit hash when the commit message references no change request.
    #[must_use]
    pub fn credits(&self, path: &str, forge: Forge) -> Vec<ListedCredit> {
        let indices = credited(&self.commits, &[path]).pop().unwrap_or_default();

        let mut credits = vec![];
        for index in indices {
            let Some(commit) = self.commits.get(index) else {
                continue;
            };
            let credit = match forge.change_request(&commit.message) {
                Some(number) => ListedCredit::ChangeRequest(number),
                None => ListedCredit::Commit(commit.hash.clone()),
            };
            if !credits.contains(&credit) {
                credits.push(credit);
            }
        }

        credits
    }

    /// Track `path` forward from `commits[index]` to `HEAD` across renames,
    /// returning `None` if a newer commit deleted the file.
    fn follow(&self, index: usize, path: &str) -> Option<String> {
        let mut path = path.to_owned();

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

/// A first-parent commit that modified the fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit {
    pub hash: String,
    pub message: String,
    pub changes: Vec<Change>,
}

/// A file change within the fragments directory relative to a commit's first
/// parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Change {
    Added(String),
    Modified(String),
    Deleted(String),
    Renamed { from: String, to: String },
}

impl Change {
    /// Return `(previous_path, current_path)` for this change, or `None` if the
    /// file was deleted.
    fn paths(&self) -> Option<(Option<&String>, &String)> {
        match self {
            Self::Added(path) => Some((None, path)),
            Self::Modified(path) => Some((Some(path), path)),
            Self::Renamed { from, to } => Some((Some(from), to)),
            Self::Deleted(_) => None,
        }
    }
}

/// Parse the NUL-delimited `--name-status -z` output of [`git::log`].
pub(crate) fn parse(log: &str) -> Result<Vec<Commit>, String> {
    let mut commits: Vec<Commit> = vec![];
    let mut fields = log.split('\0');

    while let Some(field) = fields.next() {
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
            // Treat a copied file as newly added at its destination so its
            // history starts at the copy commit.
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

/// Parse the status letter from a `--name-status` token (e.g. `'R'` from
/// `"R100"`).
fn status(field: &str) -> Option<char> {
    let mut chars = field.chars();
    let letter = chars.next().filter(char::is_ascii_uppercase)?;

    chars.all(|c| c.is_ascii_digit()).then_some(letter)
}

/// For each path in `paths` (relative to the fragments directory at `HEAD`),
/// return the indices in `commits` that added, modified, or renamed the file at
/// the top level, ordered from oldest to newest.
///
/// Follows renames backward through `commits` (which are ordered newest-first).
/// The commit that moves a file into `<version>/` and any subsequent edits
/// inside `<version>/` are excluded.
pub(crate) fn credited(commits: &[Commit], paths: &[&str]) -> Vec<Vec<usize>> {
    let mut followed: HashMap<String, usize> = paths
        .iter()
        .enumerate()
        .map(|(file, path)| ((*path).to_owned(), file))
        .collect();
    let mut credited = vec![vec![]; paths.len()];

    for (index, commit) in commits.iter().enumerate() {
        follow(commit, index, &mut followed, &mut credited);
    }

    for commits in &mut credited {
        commits.reverse();
    }

    credited
}

/// Process a single `commit` (at `index`), recording top-level changes in
/// `credited` and updating `followed` with pre-commit paths.
fn follow(
    commit: &Commit,
    index: usize,
    followed: &mut HashMap<String, usize>,
    credited: &mut [Vec<usize>],
) {
    // Stage path updates so a commit that renames `a -> b` and adds a new `a`
    // resolves each change against the post-commit paths in `followed`.
    let mut ended = vec![];
    let mut moved = vec![];

    for change in &commit.changes {
        let Some((before, after)) = change.paths() else {
            continue;
        };
        let Some(&file) = followed.get(after) else {
            continue;
        };
        if is_unreleased(after) && before.is_none_or(|before| is_unreleased(before)) {
            credited[file].push(index);
        }
        if before != Some(after) {
            ended.push(after);
            moved.extend(before.map(|before| (before, file)));
        }
    }

    for path in ended {
        followed.remove(path);
    }
    for (path, file) in moved {
        followed.insert(path.clone(), file);
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
