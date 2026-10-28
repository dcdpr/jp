//! What git knows about the files `jp vite sync` would change.
//!
//! A sync overwrites files, so it only runs where git can give them back: in a
//! work tree, on files git has recorded as they are.
//! A file with changes since the last commit, or one git doesn't track, would
//! lose something git can't restore.

use std::process::{Command, Stdio};

use camino::{Utf8Path, Utf8PathBuf};

/// Why git can't restore a file a sync would change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unrecorded {
    /// It has changes since the last commit, staged or not.
    Modified,

    /// Git doesn't track it.
    Untracked,
}

/// Where the work tree holding `root` starts, as the path from its top to
/// `root`: empty when `root` is the top, otherwise ending in `/`.
///
/// `None` when `root` isn't in a git work tree.
pub(crate) fn prefix(root: &Utf8Path) -> Result<Option<String>, String> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-prefix"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git couldn't be run: {error}"))?;

    if !output.status.success() {
        return Ok(None);
    }

    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

/// The files among `paths` git can't restore, and why, ordered by path.
///
/// `paths` are relative to `root`, and `prefix` is what [`prefix`] answered for
/// it: git reports paths from the top of the work tree.
pub(crate) fn unrecorded(
    root: &Utf8Path,
    prefix: &str,
    paths: &[Utf8PathBuf],
) -> Result<Vec<(Utf8PathBuf, Unrecorded)>, String> {
    if paths.is_empty() {
        return Ok(vec![]);
    }

    let output = Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            // An ignored file is as untracked as any other, and git has no
            // copy of it to restore. With `--untracked-files=all`, ignored
            // directories are listed file by file.
            "--ignored=traditional",
            "--no-renames",
            "--",
        ])
        .args(paths.iter().map(|path| path.as_str()))
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git couldn't be run: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "`git status` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut found = parse_status(&String::from_utf8_lossy(&output.stdout), prefix);
    found.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(found)
}

/// Read `git status --porcelain=v1 -z --no-renames --ignored` output, keeping
/// the entries below `prefix`, as paths relative to it.
///
/// An ignored file (`!!`) is read as untracked: git keeps no copy of either.
fn parse_status(output: &str, prefix: &str) -> Vec<(Utf8PathBuf, Unrecorded)> {
    output
        .split('\0')
        .filter_map(|entry| {
            let (code, path) = (entry.get(..2)?, entry.get(3..)?);
            let path = path.strip_prefix(prefix)?;
            let why = if code == "??" || code == "!!" {
                Unrecorded::Untracked
            } else {
                Unrecorded::Modified
            };

            Some((Utf8PathBuf::from(path), why))
        })
        .collect()
}

/// The `origin` remote of the work tree holding `root`, if it has one.
pub(crate) fn origin(root: &Utf8Path) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .ok()?;

    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|remote| !remote.is_empty())
}

#[cfg(test)]
#[path = "git_tests.rs"]
mod tests;
