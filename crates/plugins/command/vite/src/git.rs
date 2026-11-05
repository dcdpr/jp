//! What git knows about the `origin` remote of the work tree holding a root.
//!
//! Whether a file may be overwritten is the `overwrite` crate's to say.

use std::process::{Command, Stdio};

use camino::Utf8Path;

/// What git knows of the `origin` remote of the work tree holding a root.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Remote {
    /// Its address.
    pub origin: Option<String>,

    /// Its default branch, `main`, as `origin/HEAD` names it.
    pub branch: Option<String>,
}

/// What git knows of the `origin` remote of the work tree holding `root`.
///
/// Either part is `None` when git doesn't know it: there is no `origin`, or no
/// `origin/HEAD`, which `git remote set-head origin --auto` sets.
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

/// What a git command printed, trimmed, or `None` when it failed, couldn't be
/// run, or printed nothing.
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
