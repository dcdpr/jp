//! Whether a command may overwrite a file.
//!
//! Every command plugin that writes files into a project, such as `tools init`,
//! `tools sync`, `jp rfd init`, and `jp vite sync`, follows one rule when a
//! file it would write already exists and holds something else:
//!
//! - In a git work tree, it overwrites a file git can restore: one that is
//!   tracked and unchanged since the last commit.
//!   A file with changes since the last commit, staged or not, or one git
//!   doesn't track or ignores, is refused.
//! - A file marked assume-unchanged or skip-worktree is refused: `git status`
//!   doesn't look at it, so an edit to it is reported nowhere.
//! - A file reached through a symlink, its own or a directory's, is refused:
//!   git records where a link points, not what the target holds, and writing
//!   follows the link.
//! - Outside a git work tree, nothing can restore the file, so it is refused.
//! - `--force` overwrites a refused file anyway, writing through a symlink to
//!   its target.
//!
//! A file that doesn't exist, or already holds what the command would write,
//! loses nothing and is never refused.
//!
//! [`existing`] says what is in a file's place, [`unrecorded`] finds the files
//! to refuse, and [`refusal`] says why they were refused.

use std::{
    fmt::Write as _,
    fs, io,
    process::{Command, Stdio},
};

use camino::{Utf8Path, Utf8PathBuf};

/// What is in the place of a file a command would write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
    /// Nothing: writing the file loses nothing.
    Missing,

    /// The text the command would write, however its lines end.
    Same,

    /// Something else, which writing the file would overwrite.
    Differs,
}

/// What is in `path`'s place, compared with the `contents` a command would
/// write there.
pub fn existing(path: &Utf8Path, contents: &[u8]) -> Result<Existing, String> {
    match fs::read(path) {
        Ok(found) if same_text(&found, contents) => Ok(Existing::Same),
        Ok(_) => Ok(Existing::Differs),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Existing::Missing),
        Err(error) => Err(format!("failed to read {path}: {error}")),
    }
}

/// Why git can't restore a file a command would overwrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unrecorded {
    /// It has changes since the last commit, staged or not.
    Modified,

    /// Git doesn't track it, or ignores it.
    Untracked,

    /// It is marked assume-unchanged or skip-worktree, so `git status` doesn't
    /// report an edit to it.
    Hidden,

    /// It, or a directory above it, is a symlink: git can restore the link, not
    /// the target a write would overwrite.
    Symlink,

    /// It isn't in a git work tree.
    NoWorkTree,
}

impl Unrecorded {
    /// Why the file is refused, as a refusal lists it.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Modified => "changed since the last commit",
            Self::Untracked => "not tracked",
            Self::Hidden => "marked assume-unchanged or skip-worktree",
            Self::Symlink => "reached through a symlink",
            Self::NoWorkTree => "not in a git work tree",
        }
    }
}

/// Where the work tree holding `dir` starts, as the path from its top to `dir`:
/// empty when `dir` is the top, otherwise ending in `/`.
///
/// `None` when `dir` isn't in a git work tree, or git isn't installed.
pub fn prefix(dir: &Utf8Path) -> Result<Option<String>, String> {
    let output = match Command::new("git")
        .args(["rev-parse", "--show-prefix"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound && dir.is_dir() => return Ok(None),
        Err(error) => return Err(format!("git couldn't be run in {dir}: {error}")),
    };

    if !output.status.success() {
        return Ok(None);
    }

    Ok(Some(
        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
    ))
}

/// The files among `paths` git couldn't restore if they were overwritten, and
/// why, ordered by path.
///
/// `paths` are relative to `dir`, and name files that exist.
/// A path reached through a symlink is unrecorded wherever it is, and outside a
/// git work tree every one of them is.
/// `dir` itself may be a symlink: git reads the directory it resolves to.
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

/// Whether `path`, relative to `dir`, or a directory between `dir` and it, is a
/// symlink.
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

/// What `git status` says about `paths`, relative to `dir`, which sits at
/// `prefix` in its work tree.
fn status(
    dir: &Utf8Path,
    prefix: &str,
    paths: &[Utf8PathBuf],
) -> Result<Vec<(Utf8PathBuf, Unrecorded)>, String> {
    let output = Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            // An ignored file is as untracked as any other, and git has no copy
            // of it to restore. With `--untracked-files=all`, ignored
            // directories are listed file by file.
            "--ignored=traditional",
            "--no-renames",
            "--",
        ])
        .args(paths.iter().map(|path| path.as_str()))
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git couldn't be run in {dir}: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "`git status` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_status(
        &String::from_utf8_lossy(&output.stdout),
        prefix,
    ))
}

/// Read `git status --porcelain=v1 -z --no-renames --ignored` output, keeping
/// the entries below `prefix`, as paths relative to it.
///
/// Porcelain output names paths from the top of the work tree.
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

/// The tracked files among `paths`, relative to `dir`, which sits at `prefix`
/// in its work tree, that are marked assume-unchanged or skip-worktree.
fn hidden(
    dir: &Utf8Path,
    prefix: &str,
    paths: &[Utf8PathBuf],
) -> Result<Vec<(Utf8PathBuf, Unrecorded)>, String> {
    let output = Command::new("git")
        .args(["ls-files", "-v", "-z", "--full-name", "--"])
        .args(paths.iter().map(|path| path.as_str()))
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("git couldn't be run in {dir}: {error}"))?;

    if !output.status.success() {
        return Err(format!(
            "`git ls-files` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(parse_hidden(
        &String::from_utf8_lossy(&output.stdout),
        prefix,
    ))
}

/// Read `git ls-files -v -z --full-name` output, keeping the entries below
/// `prefix` that are assume-unchanged (a lowercase tag) or skip-worktree (`S`),
/// as paths relative to it.
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

/// The message refusing to overwrite `files`, each listed with why git couldn't
/// restore it.
///
/// `command` is the command as the user would run it again, without `--force`,
/// such as `jp vite sync`.
#[must_use]
pub fn refusal(command: &str, files: &[(Utf8PathBuf, Unrecorded)]) -> String {
    let mut message = format!("Git couldn't restore these files, so `{command}` leaves them:\n");
    for (path, why) in files {
        let _ = writeln!(message, "  {path} ({})", why.reason());
    }

    let committable = files
        .iter()
        .any(|(_, why)| matches!(why, Unrecorded::Modified | Unrecorded::Untracked));
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

/// Whether a file holds the same text as `other`, however its lines end.
///
/// Git can check text files out with CRLF line endings (`core.autocrlf`, on by
/// default in Git for Windows) and commits them back with LF, so a file that
/// differs from what a command writes only there is, to git, the same file.
/// A `\r` that doesn't end a line still counts.
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
