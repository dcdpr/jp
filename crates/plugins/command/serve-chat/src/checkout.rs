//! Locking, pulling, and write-scope checks for the bridge's git checkout.
//!
//! Files written during a turn remain uncommitted in the working tree until
//! `just chat-take` copies the diff into a review worktree and resets the
//! checkout.
//! The bridge pulls (`git pull --ff-only`) only when the working tree is clean
//! so pending changes are never clobbered, and holds a flock on
//! `jp-serve-chat.lock` during turns and pulls so `just chat-take` cannot race
//! with a running turn:
//!
//! ```sh
//! flock "$(git rev-parse --git-path jp-serve-chat.lock)" sh -c '…'
//! ```
//!
//! After each turn, the checkout checks that every modified or untracked file
//! is inside the configured writable directories; any path outside them pauses
//! the bridge.

use std::{
    fs::{File, OpenOptions},
    process::Command,
};

use camino::{Utf8Path, Utf8PathBuf};

/// Lock file name inside the repository's `.git` directory.
pub(crate) const LOCK_FILE: &str = "jp-serve-chat.lock";

/// Repository checkout lifecycle around turns.
pub(crate) trait Checkout: Send {
    /// Acquire the checkout lock for a turn and pull if the working tree is
    /// clean.
    ///
    /// Returns `Ok(Some(warning))` if the pull failed (the turn still
    /// proceeds), or `Err` if the lock could not be acquired.
    fn begin(&mut self) -> Result<Option<String>, String>;

    /// Release the lock after a turn, returning any modified or untracked paths
    /// outside the writable directories.
    fn end(&mut self) -> Result<Vec<String>, String>;

    /// Return modified or untracked paths outside the writable directories
    /// without acquiring or releasing the lock.
    fn stray(&mut self) -> Result<Vec<String>, String>;

    /// Acquire the lock and pull if the working tree is clean.
    ///
    /// Returns a warning message if locking or pulling failed.
    fn idle(&mut self) -> Option<String>;

    /// Return whether the working tree has no modified, staged, or untracked
    /// files.
    fn is_clean(&mut self) -> Result<bool, String>;
}

/// No-op [`Checkout`] implementation used when `checkout.enabled = false`.
#[derive(Debug, Default)]
pub(crate) struct Unguarded;

impl Checkout for Unguarded {
    fn begin(&mut self) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn end(&mut self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }

    fn stray(&mut self) -> Result<Vec<String>, String> {
        Ok(vec![])
    }

    fn idle(&mut self) -> Option<String> {
        None
    }

    fn is_clean(&mut self) -> Result<bool, String> {
        Ok(true)
    }
}

#[derive(Debug)]
pub(crate) struct GitCheckout {
    root: Utf8PathBuf,
    writable: Vec<String>,
    lock_path: Utf8PathBuf,

    /// Open lock file handle held between [`Checkout::begin`] and
    /// [`Checkout::end`].
    held: Option<File>,
}

impl GitCheckout {
    /// Open the git checkout at `root`, resolving the path to [`LOCK_FILE`].
    pub(crate) fn open(root: &Utf8Path, writable: Vec<String>) -> Result<Self, String> {
        let path = git(root, &["rev-parse", "--git-path", LOCK_FILE]).map_err(|error| {
            format!(
                "{root} is not a git checkout ({error}); run the bridge in one, or set \
                 `checkout.enabled = false`."
            )
        })?;
        let path = Utf8PathBuf::from(path.trim());
        let lock_path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };

        Ok(Self {
            root: root.to_owned(),
            writable,
            lock_path,
            held: None,
        })
    }

    /// Open and acquire an exclusive flock on [`Self::lock_path`], blocking
    /// until available.
    fn lock(&self) -> Result<File, String> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&self.lock_path)
            .map_err(|error| format!("failed to open {}: {error}", self.lock_path))?;
        file.lock()
            .map_err(|error| format!("failed to lock {}: {error}", self.lock_path))?;

        Ok(file)
    }

    /// Return all modified, staged, and untracked paths relative to the
    /// worktree root.
    fn changed(&self) -> Result<Vec<String>, String> {
        let status = git(&self.root, &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
        ])?;

        Ok(changed_paths(&status))
    }

    /// Run `git pull --ff-only` if the working tree is clean.
    /// Assumes the caller holds the lock.
    fn pull_if_clean(&self) -> Option<String> {
        match self.changed() {
            Ok(changed) if changed.is_empty() => {}
            Ok(_) => return None,
            Err(error) => return Some(error),
        }

        git(&self.root, &["pull", "--ff-only", "--quiet"])
            .err()
            .map(|error| format!("`git pull --ff-only` failed: {error}"))
    }
}

impl Checkout for GitCheckout {
    fn begin(&mut self) -> Result<Option<String>, String> {
        let lock = self.lock()?;
        let warning = self.pull_if_clean();
        self.held = Some(lock);

        Ok(warning)
    }

    fn end(&mut self) -> Result<Vec<String>, String> {
        let stray = self.stray();
        // Always release the lock even if `stray()` failed, so `just chat-take` does not block.
        drop(self.held.take());

        stray
    }

    fn stray(&mut self) -> Result<Vec<String>, String> {
        Ok(outside(&self.changed()?, &self.writable))
    }

    fn idle(&mut self) -> Option<String> {
        match self.lock() {
            Ok(_lock) => self.pull_if_clean(),
            Err(error) => Some(error),
        }
    }

    fn is_clean(&mut self) -> Result<bool, String> {
        self.changed().map(|changed| changed.is_empty())
    }
}

/// Run git in `root`, returning its stdout, or its stderr as the error.
fn git(root: &Utf8Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|error| format!("failed to run git: {error}"))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }

    String::from_utf8(output.stdout).map_err(|error| format!("git printed non-UTF-8: {error}"))
}

/// Width of an entry's `XY ` status prefix in `git status --porcelain=v1`.
const STATUS_PREFIX: usize = 3;

/// Parse paths from `git status --porcelain=v1 -z` output.
///
/// For renames (`R`) and copies (`C`), includes both the destination path and
/// the NUL-separated source path that follows it so a rename out of or into a
/// writable directory is checked on both sides.
pub(crate) fn changed_paths(status: &str) -> Vec<String> {
    let mut paths = vec![];
    let mut fields = status.split('\0').filter(|field| !field.is_empty());

    while let Some(entry) = fields.next() {
        let Some(path) = entry.get(STATUS_PREFIX..) else {
            continue;
        };
        paths.push(path.to_owned());

        let code = &entry[..STATUS_PREFIX - 1];
        if code.contains('R') || code.contains('C') {
            paths.extend(fields.next().map(ToOwned::to_owned));
        }
    }

    paths
}

/// Return the paths in `changed` that do not fall under any directory in
/// `writable`.
pub(crate) fn outside(changed: &[String], writable: &[String]) -> Vec<String> {
    changed
        .iter()
        .filter(|path| {
            !writable.iter().any(|dir| {
                path.as_str() == dir
                    || path
                        .strip_prefix(dir.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        })
        .cloned()
        .collect()
}

#[cfg(test)]
#[path = "checkout_tests.rs"]
mod tests;
