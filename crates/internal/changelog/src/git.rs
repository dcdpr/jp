//! Git subprocess helpers run inside the fragments directory.
//!
//! Formatting and diff options are passed explicitly on the command line so
//! user git configuration cannot alter the output.

use std::{
    fmt,
    process::{Command, Stdio},
};

use camino::Utf8Path;

use crate::history::HEADER;

/// Error returned when a `git` subprocess cannot be spawned or exits non-zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub command: String,
    pub message: String,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` failed: {}", self.command, self.message)
    }
}

impl std::error::Error for GitError {}

/// Return the first-parent `git log` of `HEAD` touching `dir`, newest commit
/// first, formatted for [`crate::history::parse`].
///
/// Merge commits diff against their first parent (`--diff-merges=first-parent`)
/// so every file change introduced by the merged branch is listed on the merge
/// commit.
/// Paths are relative to `dir`, with rename detection enabled.
pub(crate) fn log(dir: &Utf8Path) -> Result<String, GitError> {
    let format = format!("--format={HEADER}%H%n%B");

    run(dir, &[
        // Disable `log.follow`, which enables single-file `--follow` when one
        // pathspec is given.
        "-c",
        "log.follow=false",
        "log",
        "--first-parent",
        "--diff-merges=first-parent",
        "--find-renames",
        "--name-status",
        "-z",
        "--relative",
        "--encoding=UTF-8",
        "--no-color",
        "--no-show-signature",
        &format,
        "--",
        ".",
    ])
}

/// Return whether `dir` is inside a shallow git clone.
pub(crate) fn is_shallow(dir: &Utf8Path) -> Result<bool, GitError> {
    run(dir, &["rev-parse", "--is-shallow-repository"]).map(|output| output.trim() == "true")
}

/// Return the URL of the `origin` remote for `dir`, or `None` if `origin` is
/// not configured.
pub(crate) fn origin(dir: &Utf8Path) -> Option<String> {
    run(dir, &["remote", "get-url", "origin"])
        .ok()
        .map(|output| output.trim().to_owned())
        .filter(|origin| !origin.is_empty())
}

/// Return the creation date (`YYYY-MM-DD`) of `refs/tags/<tag>`, or `None` if
/// the tag does not exist.
///
/// Uses the tagger date for annotated tags and the committer date for
/// lightweight tags.
pub(crate) fn tag_date(dir: &Utf8Path, tag: &str) -> Result<Option<String>, GitError> {
    let reference = format!("refs/tags/{tag}");
    let output = run(dir, &[
        "for-each-ref",
        "--format=%(refname)%00%(creatordate:short)",
        &reference,
    ])?;

    // `for-each-ref` treats `refs/tags/<tag>` as a prefix match, so verify an
    // exact refname match to exclude nested tags under `refs/tags/<tag>/...`.
    Ok(output.lines().find_map(|line| {
        let (name, date) = line.split_once('\0')?;
        (name == reference && !date.is_empty()).then(|| date.to_owned())
    }))
}

fn run(dir: &Utf8Path, args: &[&str]) -> Result<String, GitError> {
    let failed = |message| GitError {
        command: format!("git {}", subcommand(args)),
        message,
    };

    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| failed(format!("git couldn't be run: {error}")))?;

    if !output.status.success() {
        return Err(failed(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Return the git subcommand name in `args` (the first argument that is neither
/// a flag nor a `-c key=value` pair).
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    args.iter()
        .copied()
        .find(|arg| !arg.starts_with('-') && !arg.contains('='))
        .unwrap_or_default()
}
