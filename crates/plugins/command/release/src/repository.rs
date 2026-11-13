//! Git repository operations for the release pipeline.
//!
//! Pipeline subcommands fetch `main`, `release/*`, and `release-notes/*` from
//! `origin` into remote-tracking branches, perform file and commit operations
//! in temporary worktrees (leaving the user's checkout untouched), and update
//! local branches and tags without pushing.
//!
//! [`Repository`] exposes four sub-views over a shared [`Git`] handle:
//!
//! - [`Refs`] for reading and updating branches and tags.
//! - [`History`] for inspecting commits, ancestry, and trees.
//! - [`Origin`] for fetching from `origin` and comparing the current checkout
//!   against `origin/main`.
//! - [`Worktrees`] for creating and managing temporary worktrees.
//!
//! All paths accepted or returned by these views are relative to the workspace
//! root (which may be a subdirectory of the git work tree).

mod git;
mod history;
mod origin;
mod refs;
mod worktrees;

use std::{
    collections::BTreeMap,
    env, process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};

use self::git::Git;
pub(crate) use self::{
    history::History,
    origin::Origin,
    refs::Refs,
    worktrees::{Worktree, Worktrees},
};
use crate::{
    pipeline::short,
    releases::{self, Line, RELEASE_BRANCH, Releases},
    shell::Shell,
};

pub(crate) const ORIGIN: &str = "origin";
pub(crate) const MAIN: &str = "main";
const MAIN_REF: &str = "refs/remotes/origin/main";
const ORIGIN_REFS: &str = "refs/remotes/origin/";
const LOCAL_REFS: &str = "refs/heads/";

/// Git repository handle for a workspace.
pub(crate) struct Repository<'a> {
    pub refs: Refs<'a>,
    pub history: History<'a>,
    pub origin: Origin<'a>,
    pub worktrees: Worktrees<'a>,
}

impl<'a> Repository<'a> {
    pub(crate) fn open(shell: &'a Shell, root: &Utf8Path) -> Result<Self, String> {
        let git = Git::open(shell, root)?;

        Ok(Self {
            refs: Refs::new(git.clone()),
            history: History::new(git.clone()),
            origin: Origin::new(git.clone()),
            worktrees: Worktrees::new(git),
        })
    }

    /// Load [`Releases`] from local and `origin` tags and release branches.
    pub(crate) fn releases(&self) -> Result<Releases, String> {
        self.releases_and_tags().map(|(releases, _)| releases)
    }

    /// Load [`Releases`] alongside the raw [`Tags`] map without querying
    /// `origin` for tags twice.
    pub(crate) fn releases_and_tags(&self) -> Result<(Releases, Tags), String> {
        let tags = self.refs.tags()?;
        let published = releases::versions(tags.origin.keys());
        let tagged = releases::versions(tags.local.keys())
            .union(&published)
            .cloned()
            .collect();

        let main = self.origin.main()?;
        let mut lines = BTreeMap::new();
        for branch in self.refs.branches(RELEASE_BRANCH)? {
            let Some(key) = releases::line_of(&branch.name) else {
                continue;
            };
            let Some(tip) = self.refs.tip(&branch)? else {
                continue;
            };

            let log = self.history.log_between(&main, &tip)?;
            let (started, picked) = releases::parse_line(&log)?;
            lines.insert(key, Line {
                branch: branch.name,
                tip,
                started,
                picked,
            });
        }

        let releases = Releases {
            tagged,
            published,
            lines,
        };

        Ok((releases, tags))
    }
}

/// Current workspace checkout compared against `origin/main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checkout {
    pub head: String,

    /// Checked-out branch name, or `None` when `HEAD` is detached.
    pub branch: Option<String>,

    /// Commit hash of `refs/remotes/origin/main`.
    pub main: String,
}

impl Checkout {
    pub(crate) fn at_main(&self) -> bool {
        self.head == self.main
    }

    pub(crate) fn describe(&self) -> String {
        match &self.branch {
            Some(branch) => format!("{branch} at {}", short(&self.head)),
            None => format!("the checkout at {}", short(&self.head)),
        }
    }

    /// Format the error message for a checkout that is behind `origin/main`.
    pub(crate) fn behind(&self) -> String {
        let how = if self.branch.as_deref() == Some(MAIN) {
            "`git pull`".to_owned()
        } else {
            format!("`git merge {ORIGIN}/{MAIN}`")
        };

        format!(
            "{} lacks commits {ORIGIN}'s {MAIN} has, at {}; bring them in first, such as with \
             {how}.",
            self.describe(),
            short(&self.main)
        )
    }

    /// Format a prefix clause noting that `self.head` is ahead of
    /// `origin/main`, or return `None` when `self.head == self.main`.
    pub(crate) fn ahead(&self) -> Option<String> {
        (!self.at_main()).then(|| {
            format!(
                "{} is ahead of {ORIGIN}'s {MAIN} at {}",
                self.describe(),
                short(&self.main)
            )
        })
    }
}

/// Local and `origin` `v*` tag maps (`tag_name -> commit_hash`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tags {
    pub local: BTreeMap<String, String>,
    pub origin: BTreeMap<String, String>,
}

/// Local and `origin` commit hashes for a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Branch {
    pub name: String,
    pub local: Option<String>,
    pub origin: Option<String>,
}

/// Generate a unique path in the system temporary directory for `purpose`.
pub(crate) fn temporary(purpose: &str) -> Result<Utf8PathBuf, String> {
    static TAKEN: AtomicU64 = AtomicU64::new(0);

    let dir = Utf8PathBuf::from_path_buf(env::temp_dir()).map_err(|dir| {
        format!(
            "The temporary directory {} isn't a UTF-8 path.",
            dir.display()
        )
    })?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let count = TAKEN.fetch_add(1, Ordering::Relaxed);

    Ok(dir.join(format!(
        "jp-release-{purpose}-{}-{now}-{count}",
        process::id()
    )))
}

#[cfg(test)]
#[path = "repository_tests.rs"]
mod tests;
