//! What git knows about the files a page cites.
//!
//! A permalink names a file at a commit, so checking one, or pinning a link to
//! one, asks git: whether the commit exists and is reachable from the default
//! branch, what the file held there, and which commit last held what the work
//! tree holds now.
//! Every command runs in the workspace root.

use std::process::{Command, Stdio};

use camino::Utf8Path;

/// The default branch, as the remote reports it.
pub(crate) const DEFAULT_BRANCH: &str = "refs/remotes/origin/HEAD";

/// What a successful git command printed, or `None` when it failed or couldn't
/// be run.
fn output(root: &Utf8Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .ok()?;

    output.status.success().then_some(output.stdout)
}

/// What a successful git command printed, trimmed, or `None`.
fn text(root: &Utf8Path, args: &[&str]) -> Option<String> {
    output(root, args)
        .map(|stdout| String::from_utf8_lossy(&stdout).trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The `origin` remote of the work tree holding `root`, if it has one.
pub(crate) fn origin(root: &Utf8Path) -> Option<String> {
    text(root, &["remote", "get-url", "origin"])
}

/// The path from the top of the work tree to `root`: empty when `root` is the
/// top, otherwise ending in `/`.
///
/// `None` when `root` isn't in a git work tree.
pub(crate) fn prefix(root: &Utf8Path) -> Option<String> {
    output(root, &["rev-parse", "--show-prefix"])
        .map(|stdout| String::from_utf8_lossy(&stdout).trim().to_owned())
}

/// The object `rev` names, as a full hash.
pub(crate) fn resolve(root: &Utf8Path, rev: &str) -> Option<String> {
    text(root, &["rev-parse", "--verify", "--quiet", rev])
}

/// Whether `commit` names a commit this repository holds.
pub(crate) fn has_commit(root: &Utf8Path, commit: &str) -> bool {
    output(root, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_some()
}

/// Whether `commit` is reachable from `branch`.
///
/// An error says why that can't be told, such as a branch the repository
/// doesn't have.
pub(crate) fn is_ancestor(root: &Utf8Path, commit: &str, branch: &str) -> Result<bool, String> {
    let status = Command::new("git")
        .args(["merge-base", "--is-ancestor", commit, branch])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("git couldn't be run: {error}"))?;

    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("git can't tell whether {commit} is on {branch}")),
    }
}

/// What kind of object `rev` (`<commit>:<path>`) names: `blob`, `tree`, or
/// `None` when it names nothing.
pub(crate) fn object_type(root: &Utf8Path, rev: &str) -> Option<String> {
    text(root, &["cat-file", "-t", rev])
}

/// The contents of the blob `rev` (`<commit>:<path>`) names.
pub(crate) fn blob(root: &Utf8Path, rev: &str) -> Option<String> {
    output(root, &["cat-file", "blob", rev])
        .map(|stdout| String::from_utf8_lossy(&stdout).into_owned())
}

/// The hash the file at `path` would have as a blob, as the work tree holds it.
pub(crate) fn hash_object(root: &Utf8Path, path: &str) -> Option<String> {
    text(root, &["hash-object", "--", path])
}

/// The commits on `branch` that changed `path`, newest first.
pub(crate) fn changes(root: &Utf8Path, branch: &str, path: &str) -> Vec<String> {
    text(root, &["log", "--format=%H", branch, "--", path])
        .map(|log| log.lines().map(ToOwned::to_owned).collect())
        .unwrap_or_default()
}

/// Whether the work tree holds `path`, a directory, as `rev` does: no change to
/// a tracked file, and no file git doesn't track.
pub(crate) fn unchanged_since(root: &Utf8Path, rev: &str, path: &str) -> bool {
    output(root, &["diff", "--quiet", rev, "--", path]).is_some()
        && output(root, &["status", "--porcelain", "--", path])
            .is_some_and(|status| status.is_empty())
}
