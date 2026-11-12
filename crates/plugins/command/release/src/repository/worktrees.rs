//! Worktrees of the repository, where the release commands write, leaving the
//! checkout they run from alone.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use semver::Version;

use super::{git::Git, temporary};
use crate::{
    options::{BUMP_KEY, Bump},
    shell::Ran,
};

/// The repository's worktrees.
pub(crate) struct Worktrees<'a> {
    git: Git<'a>,
}

impl<'a> Worktrees<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    /// The git directory the repository's worktrees share.
    pub(crate) fn common_dir(&self) -> Result<Utf8PathBuf, String> {
        let dir = Utf8PathBuf::from(self.git.git(&["rev-parse", "--git-common-dir"])?.trim());

        Ok(if dir.is_absolute() {
            dir
        } else {
            self.git.root.join(dir)
        })
    }

    /// A new worktree at the commit `hash`, named after `purpose`, which is
    /// removed when dropped.
    pub(crate) fn add(&self, purpose: &str, hash: &str) -> Result<Worktree<'a>, String> {
        let top = temporary(purpose)?;
        self.git
            .git(&["worktree", "add", "--detach", "--quiet", top.as_str(), hash])?;

        Ok(self.open(top))
    }

    /// The worktree at `top` a stopped run kept, to finish it.
    ///
    /// The error says there is no worktree there.
    pub(crate) fn reopen(&self, top: &Utf8Path) -> Result<Worktree<'a>, String> {
        if !top.join(".git").exists() {
            return Err(format!("There is no worktree at {top}."));
        }

        Ok(self.open(top.to_owned()))
    }

    fn open(&self, top: Utf8PathBuf) -> Worktree<'a> {
        Worktree {
            workspace: top.join(&self.git.prefix),
            git: self.git.clone(),
            top,
            kept: false,
        }
    }
}

/// A worktree of the repository, removed when dropped unless kept.
pub(crate) struct Worktree<'a> {
    git: Git<'a>,

    /// The top of the worktree.
    top: Utf8PathBuf,

    /// The workspace root in this worktree.
    workspace: Utf8PathBuf,

    kept: bool,
}

impl Worktree<'_> {
    /// The top of the worktree.
    pub(crate) fn top(&self) -> &Utf8Path {
        &self.top
    }

    /// The workspace root in this worktree.
    pub(crate) fn workspace(&self) -> Utf8PathBuf {
        self.workspace.clone()
    }

    /// Run git with `args` in the workspace root in this worktree.
    pub(crate) fn git(&self, args: &[&str]) -> Result<String, String> {
        self.git.shell.git(&self.workspace, args)
    }

    /// Run git with `args` in the workspace root in this worktree, whatever it
    /// exits with.
    pub(crate) fn try_git(&self, args: &[&str]) -> Result<Ran, String> {
        self.git.shell.try_git(&self.workspace, args)
    }

    /// The hash of the commit the worktree is at.
    pub(crate) fn head(&self) -> Result<String, String> {
        Ok(self.git(&["rev-parse", "HEAD"])?.trim().to_owned())
    }

    /// Commit everything changed in the worktree with `message`, and return the
    /// commit's hash.
    ///
    /// Commits even when nothing changed.
    pub(crate) fn commit(&self, message: &str) -> Result<String, String> {
        self.git(&["add", "--all"])?;
        self.git(&["commit", "--quiet", "--allow-empty", "--message", message])?;

        self.head()
    }

    /// Run `bump` in the workspace root in this worktree, to write `version`.
    ///
    /// The error names the setting `bump` comes from, and quotes what it
    /// printed.
    pub(crate) fn bump(&self, bump: &Bump, version: &Version) -> Result<(), String> {
        let (program, args) = bump.command(version);
        let ran = self
            .git
            .shell
            .run(&self.workspace, &program, &args)
            .map_err(|error| format!("`{BUMP_KEY}` couldn't write {version}: {error}"))?;
        if !ran.success {
            return Err(format!(
                "`{BUMP_KEY}` failed writing {version}: {}",
                ran.said()
            ));
        }

        Ok(())
    }

    /// Keep the worktree, and return where it is.
    pub(crate) fn keep(mut self) -> Utf8PathBuf {
        self.kept = true;
        self.top.clone()
    }
}

impl Drop for Worktree<'_> {
    fn drop(&mut self) {
        if self.kept {
            return;
        }

        // Forced, so a worktree holding changes, such as a pick a conflict
        // stopped, goes too.
        drop(
            self.git
                .git(&["worktree", "remove", "--force", self.top.as_str()]),
        );
        if self.top.exists() {
            drop(fs::remove_dir_all(&self.top));
            drop(self.git.git(&["worktree", "prune"]));
        }
    }
}
