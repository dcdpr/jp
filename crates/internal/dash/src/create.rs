//! Worktree creation for pull requests and new branches.
//!
//! New worktrees are created as siblings of the current worktree, named after
//! the branch, starting from the local default branch (`main`) after
//! fast-forwarding it from upstream.
//! If fast-forwarding fails (for example, when offline or when `main` has
//! diverged), the worktree is created from the local `main` as-is and
//! [`Created::stale`] records the reason.
//!
//! For a pull request, the worktree is created detached and then populated with
//! `gh pr checkout` so fork remotes are configured automatically.
//! If the checkout fails, the detached worktree is removed.

use camino::{Utf8Path, Utf8PathBuf};

use crate::source::{git, github};

/// Result of creating a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub path: Utf8PathBuf,

    /// Reason why fast-forwarding the default branch failed, if it did.
    pub stale: Option<String>,
}

/// Create a sibling worktree for pull request `number` (with head branch
/// `branch`) in the repository containing `dir`.
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
        // Clean up the newly created detached worktree on checkout failure.
        let _leftover = git::remove_worktree(dir, &path);
        return Err(format!("Checking out #{number} failed: {error}"));
    }

    Ok(Created { path, stale })
}

/// Create a sibling worktree on a new branch `branch`, starting from the
/// default branch of the repository containing `dir`.
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

/// Return the parent directory of the worktree containing `dir`.
fn parent(dir: &Utf8Path) -> Result<Utf8PathBuf, String> {
    let root =
        git::toplevel(dir).map_err(|error| format!("Finding the worktree failed: {error}"))?;
    root.parent()
        .map(Utf8Path::to_owned)
        .ok_or_else(|| format!("{root} has no parent directory for a new worktree."))
}

/// Fast-forward the default branch and return its name along with any update
/// error.
fn update_base(dir: &Utf8Path) -> (String, Option<String>) {
    let base = git::default_branch(dir);
    let stale = git::update_branch(dir, &base)
        .err()
        .map(|error| format!("{base} could not be updated ({error})"));
    (base, stale)
}

/// Sanitize `branch` into a directory name, replacing characters outside
/// `[A-Za-z0-9._-]` with `-` and trimming leading or trailing `-` and `.`.
///
/// Returns `None` if the resulting name is empty.
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
