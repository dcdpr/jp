//! File-overwrite checks for command plugins.
//!
//! Command plugins that write files into a project (`tools init`, `tools sync`,
//! `jp rfd init`, `jp vite init`, and `jp vite sync`) share one rule when a
//! target file already exists with different contents:
//!
//! - In a git work tree, tracked files with no changes since `HEAD` can be
//!   restored by git and are overwritten.
//!   Modified (staged or unstaged), untracked, and ignored files are refused.
//! - Files marked `assume-unchanged` or `skip-worktree` are refused because
//!   `git status` ignores local edits to them.
//! - Files reached through a symlink (the file itself or an ancestor directory)
//!   are refused because git tracks only the link path, not the target's
//!   contents, and writing follows the symlink.
//! - Outside a git work tree, existing files cannot be restored by git and are
//!   refused.
//! - `--force` overwrites refused files anyway, following symlinks to their
//!   targets.
//!
//! Missing files and files that already match the target contents (ignoring
//! `CRLF` vs. `LF` line endings) are never refused.
//!
//! [`existing`] compares a file on disk against the target contents,
//! [`unrecorded`] finds existing files that git cannot restore, and [`refusal`]
//! formats the error message.

use std::{
    fmt::Write as _,
    fs, io,
    process::{Command, Output, Stdio},
};

use camino::{Utf8Path, Utf8PathBuf};

/// State of a target path on disk relative to the contents to be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// The file does not exist.
    Missing,

    /// The file already has the target contents (ignoring `CRLF` vs. `LF` line
    /// endings).
    Same,

    /// The file exists with different contents.
    Differs,
}

/// Compare the file at `path` against `contents`.
pub fn existing(path: &Utf8Path, contents: &[u8]) -> Result<Existing, String> {
    match fs::read(path) {
        Ok(found) if same_text(&found, contents) => Ok(Existing::Same),
        Ok(_) => Ok(Existing::Differs),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Existing::Missing),
        Err(error) => Err(format!("failed to read {path}: {error}")),
    }
}

/// Reason git cannot restore a file if it is overwritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unrecorded {
    /// The file has staged or unstaged changes since `HEAD`.
    Modified,

    /// The file is untracked or ignored by git.
    Untracked,

    /// The file is marked `assume-unchanged` or `skip-worktree`, so `git
    /// status` does not report edits to it.
    Hidden,

    /// The file or one of its parent directories is a symlink; git tracks the
    /// link path, not the target's contents.
    Symlink,

    /// The file is not inside a git work tree.
    NoWorkTree,
}

impl Unrecorded {
    /// Return the reason label for [`refusal`] and whether committing the file
    /// would make it restorable.
    fn describe(self) -> (&'static str, bool) {
        match self {
            Self::Modified => ("changed since the last commit", true),
            Self::Untracked => ("not tracked", true),
            Self::Hidden => ("marked assume-unchanged or skip-worktree", false),
            Self::Symlink => ("reached through a symlink", false),
            Self::NoWorkTree => ("not in a git work tree", false),
        }
    }
}

/// Return the path of `dir` relative to the git work tree root (empty at the
/// root, or ending with `/` in a subdirectory).
///
/// Returns `None` if `dir` is outside a git work tree or `git` is not
/// installed.
pub fn prefix(dir: &Utf8Path) -> Result<Option<String>, String> {
    let output = match git(dir, ["rev-parse", "--show-prefix"]) {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound && dir.is_dir() => return Ok(None),
        Err(error) => return Err(not_run(dir, &error)),
    };

    if !output.status.success() {
        return Ok(None);
    }

    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

/// Return the files in `paths` that git cannot restore if overwritten, sorted
/// by path.
///
/// Each path in `paths` must be relative to `dir` and point to an existing
/// file.
/// Any path traversing a symlink within `dir` is reported as
/// [`Unrecorded::Symlink`], and outside a git work tree all remaining paths are
/// reported as [`Unrecorded::NoWorkTree`].
/// `dir` itself may be a symlink; git operates on its resolved directory.
pub fn unrecorded(
    dir: &Utf8Path,
    paths: &[Utf8PathBuf],
) -> Result<Vec<(Utf8PathBuf, Unrecorded)>, String> {
    let mut found = vec![];
    let mut rest = vec![];
    for path in paths {
        if through_symlink(dir, path)? {
            found.push((path.clone(), Unrecorded::Symlink));
        } else {
            rest.push(path.clone());
        }
    }
    if !rest.is_empty() {
        match prefix(dir)? {
            Some(prefix) => {
                found.extend(status(dir, &prefix, &rest)?);
                for entry in hidden(dir, &prefix, &rest)? {
                    if !found.iter().any(|(path, _)| *path == entry.0) {
                        found.push(entry);
                    }
                }
            }
            None => found.extend(rest.into_iter().map(|path| (path, Unrecorded::NoWorkTree))),
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(found)
}

/// Return whether `path` (relative to `dir`) or any intermediate directory
/// under `dir` is a symlink.
fn through_symlink(dir: &Utf8Path, path: &Utf8Path) -> Result<bool, String> {
    let mut current = dir.to_path_buf();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("failed to read {current}: {error}")),
        }
    }

    Ok(false)
}

fn git<'a>(dir: &Utf8Path, args: impl IntoIterator<Item = &'a str>) -> io::Result<Output> {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
}

fn not_run(dir: &Utf8Path, error: &io::Error) -> String {
    format!("git couldn't be run in {dir}: {error}")
}

/// Run `git` in `dir` with `args` and `paths`, returning stdout or an error
/// naming the subcommand (`args[0]`).
fn read(dir: &Utf8Path, args: &[&str], paths: &[Utf8PathBuf]) -> Result<String, String> {
    let all = args
        .iter()
        .copied()
        .chain(paths.iter().map(|path| path.as_str()));
    let output = git(dir, all).map_err(|error| not_run(dir, &error))?;

    if !output.status.success() {
        return Err(format!(
            "`git {}` failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Check `paths` (relative to `dir` at work-tree `prefix`) with `git status`.
fn status(
    dir: &Utf8Path,
    prefix: &str,
    paths: &[Utf8PathBuf],
) -> Result<Vec<(Utf8PathBuf, Unrecorded)>, String> {
    let args = [
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        // Include ignored files and expand ignored directories
        // (`--untracked-files=all`) so ignored files are reported as untracked.
        "--ignored=traditional",
        "--no-renames",
        "--",
    ];

    Ok(parse_status(&read(dir, &args, paths)?, prefix))
}

/// Parse `git status --porcelain=v1 -z --no-renames --ignored` output into
/// paths relative to `prefix`.
///
/// Porcelain paths are relative to the work tree root.
/// Both untracked (`??`) and ignored (`!!`) files map to
/// [`Unrecorded::Untracked`].
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

/// Find tracked files in `paths` that are marked `assume-unchanged` or
/// `skip-worktree`.
fn hidden(
    dir: &Utf8Path,
    prefix: &str,
    paths: &[Utf8PathBuf],
) -> Result<Vec<(Utf8PathBuf, Unrecorded)>, String> {
    let args = ["ls-files", "-v", "-z", "--full-name", "--"];

    Ok(parse_hidden(&read(dir, &args, paths)?, prefix))
}

/// Parse `git ls-files -v -z --full-name` output, returning entries under
/// `prefix` marked `assume-unchanged` (lowercase tag) or `skip-worktree` (`S`).
fn parse_hidden(output: &str, prefix: &str) -> Vec<(Utf8PathBuf, Unrecorded)> {
    output
        .split('\0')
        .filter_map(|entry| {
            let (tag, path) = entry.split_once(' ')?;
            let hidden = tag.chars().all(|c| c.is_ascii_lowercase()) || tag == "S";
            if tag.is_empty() || !hidden {
                return None;
            }
            let path = path.strip_prefix(prefix)?;

            Some((Utf8PathBuf::from(path), Unrecorded::Hidden))
        })
        .collect()
}

/// Format an error message refusing to overwrite `files`, listing why git
/// cannot restore each one.
///
/// `command` is the invocation without `--force` (e.g. `jp vite sync`).
#[must_use]
pub fn refusal(command: &str, files: &[(Utf8PathBuf, Unrecorded)]) -> String {
    let mut message = format!("Git couldn't restore these files, so `{command}` leaves them:\n");
    let mut committable = false;
    for (path, why) in files {
        let (reason, fixed_by_committing) = why.describe();
        committable |= fixed_by_committing;
        let _ = writeln!(message, "  {path} ({reason})");
    }

    if committable {
        let _ = write!(
            message,
            "Commit them, or run `{command} --force` to overwrite them anyway."
        );
    } else {
        let _ = write!(message, "Run `{command} --force` to overwrite them anyway.");
    }

    message
}

/// Return whether `file` and `other` have identical contents when `CRLF` is
/// normalized to `LF`.
///
/// Git can check out text files with `CRLF` line endings (`core.autocrlf`,
/// enabled by default in Git for Windows) while storing `LF` in the index, so a
/// file that differs only in `CRLF` vs. `LF` line endings is unchanged in git.
/// Standalone `\r` bytes not followed by `\n` are preserved.
#[must_use]
pub fn same_text(file: &[u8], other: &[u8]) -> bool {
    fn lf(bytes: &[u8]) -> impl Iterator<Item = u8> + '_ {
        bytes
            .iter()
            .enumerate()
            .filter(|&(index, &byte)| byte != b'\r' || bytes.get(index + 1) != Some(&b'\n'))
            .map(|(_, &byte)| byte)
    }

    lf(file).eq(lf(other))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
