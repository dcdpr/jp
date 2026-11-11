//! Git remote discovery for the workspace work tree.

use std::process::{Command, Stdio};

use camino::Utf8Path;

/// URL and default branch of the `origin` remote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Remote {
    /// Remote URL from `git remote get-url origin`.
    pub origin: Option<String>,

    /// Default branch name from `refs/remotes/origin/HEAD` (e.g. `"main"`).
    pub branch: Option<String>,
}

/// Read the `origin` remote URL and default branch for `root`.
///
/// Fields are `None` if `origin` or `origin/HEAD` is not configured (see `git
/// remote set-head origin --auto`).
pub(crate) fn remote(root: &Utf8Path) -> Remote {
    let branch = run(root, &[
        "symbolic-ref",
        "--short",
        "refs/remotes/origin/HEAD",
    ])
    .and_then(|head| head.strip_prefix("origin/").map(ToOwned::to_owned));

    Remote {
        origin: run(root, &["remote", "get-url", "origin"]),
        branch,
    }
}

/// Run `git` in `root` with `args` and return trimmed non-empty stdout, or
/// `None` on failure.
fn run(root: &Utf8Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .ok()?;

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|printed| !printed.is_empty())
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
