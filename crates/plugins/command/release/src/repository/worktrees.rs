//! Temporary git worktrees used by release pipeline commands to stage commits
//! without modifying the user's checkout.

use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use semver::Version;

use super::{git::Git, temporary};
use crate::{
    options::{BUMP_KEY, Bump},
    shell::Ran,
};

pub(crate) struct Worktrees<'a> {
    git: Git<'a>,
}

impl<'a> Worktrees<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    /// Return the repository's shared `.git` directory (`--git-common-dir`).
    pub(crate) fn common_dir(&self) -> Result<Utf8PathBuf, String> {
        let dir = Utf8PathBuf::from(self.git.git(&["rev-parse", "--git-common-dir"])?.trim());

        Ok(if dir.is_absolute() {
            dir
        } else {
            self.git.root.join(dir)
        })
    }

    /// Create a detached worktree at `hash` that is removed on drop unless
    /// [`Worktree::keep`] is called.
    pub(crate) fn add(&self, purpose: &str, hash: &str) -> Result<Worktree<'a>, String> {
        let top = temporary(purpose)?;
        self.git
            .git(&["worktree", "add", "--detach", "--quiet", top.as_str(), hash])?;

        Ok(self.open(top))
    }

    /// Reopen an existing worktree at `top` left behind by a paused command.
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

/// Temporary git worktree removed on drop unless [`Self::keep`] is called.
pub(crate) struct Worktree<'a> {
    git: Git<'a>,
    top: Utf8PathBuf,
    workspace: Utf8PathBuf,
    kept: bool,
}

impl Worktree<'_> {
    pub(crate) fn top(&self) -> &Utf8Path {
        &self.top
    }

    pub(crate) fn workspace(&self) -> Utf8PathBuf {
        self.workspace.clone()
    }

    pub(crate) fn git(&self, args: &[&str]) -> Result<String, String> {
        self.git.shell.git(&self.workspace, args)
    }

    pub(crate) fn try_git(&self, args: &[&str]) -> Result<Ran, String> {
        self.git.shell.try_git(&self.workspace, args)
    }

    pub(crate) fn head(&self) -> Result<String, String> {
        Ok(self.git(&["rev-parse", "HEAD"])?.trim().to_owned())
    }

    /// Stage all changes in the worktree, create a commit with `message`, and
    /// return the new commit hash.
    pub(crate) fn commit(&self, message: &str) -> Result<String, String> {
        self.git(&["add", "--all"])?;
        self.git(&["commit", "--quiet", "--allow-empty", "--message", message])?;

        self.head()
    }

    /// Run the configured `bump` command in the worktree's workspace root.
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

    /// Prevent the worktree from being removed on drop and return its root
    /// path.
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
