//! Creating worktrees: one for a pull request that has none, or one on a new
//! branch.
//!
//! A new worktree goes next to the one the dashboard runs in, named after its
//! branch, and starts from the local default branch (`main`), brought up to
//! date first.
//! When that update fails (offline, or `main` has diverged), the worktree still
//! starts from `main` as it is, and the result says so.
//!
//! A pull request's worktree starts detached, and `gh pr checkout` then puts
//! the pull request's branch in it, fork remotes and all.
//! A worktree whose checkout fails is removed again, so a failed attempt leaves
//! nothing behind.

use camino::{Utf8Path, Utf8PathBuf};

use crate::source::{git, github};

/// A worktree that was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub path: Utf8PathBuf,

    /// Why the default branch could not be updated first, if it could not.
    pub stale: Option<String>,
}

/// Create a worktree for pull request `number`, whose head is `branch`.
/// `dir` is any directory of the repository.
///
/// Errors are status-line messages.
pub fn for_pull_request(dir: &Utf8Path, number: u64, branch: &str) -> Result<Created, String> {
    let parent = parent(dir)?;
    let name = directory_name(branch).unwrap_or_else(|| format!("pr-{number}"));
    let path = [name.clone(), format!("{name}-pr-{number}")]
        .into_iter()
        .map(|name| parent.join(name))
        .find(|path| !path.exists())
        .ok_or_else(|| format!("{name} and {name}-pr-{number} already exist in {parent}."))?;

    let (base, stale) = update_base(dir);
    git::add_detached(dir, &path, &base)
        .map_err(|error| format!("Adding {path} failed: {error}"))?;

    if let Err(error) = github::checkout(&path, number) {
        // The worktree holds nothing but a detached checkout; nothing is lost.
        let _leftover = git::remove_worktree(dir, &path);
        return Err(format!("Checking out #{number} failed: {error}"));
    }

    Ok(Created { path, stale })
}

/// Create a worktree on a new branch `branch`, starting from the default
/// branch.
/// `dir` is any directory of the repository.
///
/// Errors are status-line messages.
pub fn for_branch(dir: &Utf8Path, branch: &str) -> Result<Created, String> {
    if branch.starts_with('-') || git::check_branch_name(dir, branch).is_err() {
        return Err(format!("`{branch}` is not a valid branch name."));
    }

    let parent = parent(dir)?;
    let name =
        directory_name(branch).ok_or_else(|| format!("`{branch}` makes no directory name."))?;
    let path = parent.join(&name);
    if path.exists() {
        return Err(format!("{path} already exists."));
    }

    let (base, stale) = update_base(dir);
    git::add_branch(dir, &path, branch, &base)
        .map_err(|error| format!("Adding {path} failed: {error}"))?;

    Ok(Created { path, stale })
}

/// The directory new worktrees go in: the parent of the worktree containing
/// `dir`.
fn parent(dir: &Utf8Path) -> Result<Utf8PathBuf, String> {
    let root =
        git::toplevel(dir).map_err(|error| format!("Finding the worktree failed: {error}"))?;
    root.parent()
        .map(Utf8Path::to_owned)
        .ok_or_else(|| format!("{root} has no parent directory for a new worktree."))
}

/// Bring the default branch up to date, and return it with why that failed, if
/// it did.
fn update_base(dir: &Utf8Path) -> (String, Option<String>) {
    let base = git::default_branch(dir);
    let stale = git::update_branch(dir, &base)
        .err()
        .map(|error| format!("{base} could not be updated ({error})"));
    (base, stale)
}

/// A directory name for `branch`: path separators and anything else outside
/// letters, digits, `-`, `_` and `.` become `-`.
/// `None` when nothing but separators and dots is left.
fn directory_name(branch: &str) -> Option<String> {
    let name: String = branch
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let name = name.trim_matches(|c| c == '-' || c == '.');

    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
#[path = "create_tests.rs"]
mod tests;
