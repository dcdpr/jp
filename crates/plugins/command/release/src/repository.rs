//! The repository holding the workspace, as `cut`, `pick`, `finalize`, and
//! `due` reach it.
//!
//! They read what `origin` holds after fetching its `main`, release branches,
//! and notes branches into remote-tracking branches, together with the release
//! branches, notes branches, and tags this repository has: where both have a
//! branch, the one with the other's commits counts (see [`Standing`]).
//! They write in worktrees of their own, so the checkout they run from is left
//! alone, and keep what they make in this repository's branches and tags.
//! Nothing is pushed.
//! The default branch is `main`.
//!
//! [`Repository`] reaches the repository through four views, which run git the
//! same way: [`Refs`] for branches and tags, [`History`] for commits and what
//! they hold, [`Origin`] for `origin` and the checkout measured against it, and
//! [`Worktrees`].
//! Every path a view takes or gives is relative to the workspace root, which
//! may sit below the top of the work tree.
//!
//! [`Standing`]: crate::releases::Standing

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

/// The remote releases are read from, and published to.
pub(crate) const ORIGIN: &str = "origin";

/// The default branch, which every change reaches first.
pub(crate) const MAIN: &str = "main";

/// Where `main` is fetched to.
const MAIN_REF: &str = "refs/remotes/origin/main";

/// Where `origin`'s branches are fetched to, each below its own name.
const ORIGIN_REFS: &str = "refs/remotes/origin/";

/// Where this repository's branches are.
const LOCAL_REFS: &str = "refs/heads/";

/// The repository holding a workspace.
pub(crate) struct Repository<'a> {
    pub refs: Refs<'a>,
    pub history: History<'a>,
    pub origin: Origin<'a>,
    pub worktrees: Worktrees<'a>,
}

impl<'a> Repository<'a> {
    /// The repository holding the workspace at `root`.
    ///
    /// The error says the workspace isn't in a git repository.
    pub(crate) fn open(shell: &'a Shell, root: &Utf8Path) -> Result<Self, String> {
        let git = Git::open(shell, root)?;

        Ok(Self {
            refs: Refs::new(git.clone()),
            history: History::new(git.clone()),
            origin: Origin::new(git.clone()),
            worktrees: Worktrees::new(git),
        })
    }

    /// What the tags and release branches here and on `origin` say, each
    /// release branch at its tip (see [`Refs::tip`]).
    pub(crate) fn releases(&self) -> Result<Releases, String> {
        self.releases_and_tags().map(|(releases, _)| releases)
    }

    /// [`releases`], and the tags they were read from, for a command that needs
    /// both without asking `origin` for the tags twice.
    ///
    /// [`releases`]: Self::releases
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

/// The commit the workspace's checkout is at, which `cut`, `pick`, and
/// `finalize` read as `main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checkout {
    /// The commit's hash.
    pub head: String,

    /// The branch checked out; `None` on a detached `HEAD`.
    pub branch: Option<String>,

    /// The hash of `origin`'s `main`, as last fetched.
    pub main: String,
}

impl Checkout {
    /// Whether the checkout is at `origin`'s `main`.
    pub(crate) fn at_main(&self) -> bool {
        self.head == self.main
    }

    /// Where the checkout is, as a sentence names it: `main at 4c5d6e7`.
    pub(crate) fn describe(&self) -> String {
        match &self.branch {
            Some(branch) => format!("{branch} at {}", short(&self.head)),
            None => format!("the checkout at {}", short(&self.head)),
        }
    }

    /// That the checkout lacks commits of `origin`'s `main`, and how to bring
    /// them in: `git pull` on `main`, and `git merge origin/main` on any other
    /// branch, where `git pull` brings in the branch's own upstream.
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

    /// That the checkout is ahead of `origin`'s `main`, as the start of a
    /// sentence, when it is.
    ///
    /// Holds only for a checkout with every commit of `origin`'s `main`.
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

/// The tags this repository and `origin` each have: each tag's name, such as
/// `v0.3.0`, and the hash of the commit it names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tags {
    pub local: BTreeMap<String, String>,
    pub origin: BTreeMap<String, String>,
}

/// A branch as this repository and `origin` each hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Branch {
    /// Its name, such as `release/0.3`.
    pub name: String,

    /// The hash of this repository's branch, when it has one.
    pub local: Option<String>,

    /// The hash of `origin`'s branch, as last fetched, when it has one.
    pub origin: Option<String>,
}

/// A path in the temporary directory nothing else takes, named after `purpose`.
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
