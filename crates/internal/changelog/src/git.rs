//! The git commands a release is read with, each run in the fragments
//! directory.
//!
//! Every setting that changes what git writes is pinned on the command line, so
//! a user's git configuration can't change what is read.

use std::{
    fmt,
    process::{Command, Stdio},
};

use camino::Utf8Path;

use crate::history::HEADER;

/// A git command that couldn't be run, or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    /// The command, as `git log`.
    pub command: String,

    /// Why: what git wrote to stderr, or why it couldn't be run.
    pub message: String,
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` failed: {}", self.command, self.message)
    }
}

impl std::error::Error for GitError {}

/// Every commit on the first-parent line of `HEAD` that changed the fragments
/// directory `dir`, newest first, as `history::parse` reads it.
///
/// A merge commit lists what it changed against its first parent: everything
/// its branch brought.
/// Paths are relative to `dir`, and renames within it are detected.
pub(crate) fn log(dir: &Utf8Path) -> Result<String, GitError> {
    let format = format!("--format={HEADER}%H%n%B");

    run(dir, &[
        // `log.follow` turns `--follow` on for a single path, which follows
        // one file, not a directory.
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

/// Whether the repository holding `dir` is a shallow clone, which lacks the
/// history credits are read from.
pub(crate) fn is_shallow(dir: &Utf8Path) -> Result<bool, GitError> {
    run(dir, &["rev-parse", "--is-shallow-repository"]).map(|output| output.trim() == "true")
}

/// The address of the `origin` remote of the repository holding `dir`, or
/// `None` when it has none.
pub(crate) fn origin(dir: &Utf8Path) -> Option<String> {
    run(dir, &["remote", "get-url", "origin"])
        .ok()
        .map(|output| output.trim().to_owned())
        .filter(|origin| !origin.is_empty())
}

/// The day the tag `tag` was made, as `YYYY-MM-DD` in the tagger's time zone,
/// or `None` when there is no such tag.
///
/// An annotated tag's date is when it was tagged; a lightweight tag's is its
/// commit's.
pub(crate) fn tag_date(dir: &Utf8Path, tag: &str) -> Result<Option<String>, GitError> {
    let reference = format!("refs/tags/{tag}");
    let output = run(dir, &[
        "for-each-ref",
        "--format=%(refname)%00%(creatordate:short)",
        &reference,
    ])?;

    // The pattern also matches the tags below `<tag>/`, which can't hold a
    // release.
    Ok(output.lines().find_map(|line| {
        let (name, date) = line.split_once('\0')?;
        (name == reference && !date.is_empty()).then(|| date.to_owned())
    }))
}

/// What `git` with `args`, run in `dir`, wrote to stdout.
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

/// The subcommand `args` run, such as `log`: the first argument that is neither
/// an option nor a `-c` setting.
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    args.iter()
        .copied()
        .find(|arg| !arg.starts_with('-') && !arg.contains('='))
        .unwrap_or_default()
}
