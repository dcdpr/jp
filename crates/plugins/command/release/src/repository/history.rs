//! The commits of the repository, and what they hold.

use std::collections::{BTreeSet, HashSet};

use super::{LOCAL_REFS, ORIGIN_REFS, git::Git};
use crate::releases::RELEASE_BRANCH;

/// The commits of the repository, and what they hold.
///
/// Paths are relative to the workspace root, both in what is asked and in what
/// is answered.
pub(crate) struct History<'a> {
    git: Git<'a>,
}

impl<'a> History<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    /// The full hash of the commit `revision` names.
    ///
    /// The error says it names no commit.
    pub(crate) fn commit(&self, revision: &str) -> Result<String, String> {
        self.git.commit(revision)
    }

    /// Whether this repository has the commit `hash`.
    pub(crate) fn has_commit(&self, hash: &str) -> Result<bool, String> {
        let object = format!("{hash}^{{commit}}");

        Ok(self.git.hash(&object)?.is_some())
    }

    /// Whether the commit `ancestor` is in the history of `descendant`, or is
    /// it.
    pub(crate) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        self.git.is_ancestor(ancestor, descendant)
    }

    /// The subject of the commit `hash`: the first line of its message.
    pub(crate) fn subject(&self, hash: &str) -> Result<String, String> {
        let subject = self.git.git(&["log", "-1", "--format=%s", hash])?;

        Ok(subject.trim().to_owned())
    }

    /// The first-parent line of the commit `hash`, newest first.
    pub(crate) fn first_parents(&self, hash: &str) -> Result<Vec<String>, String> {
        let listing = self.git.git(&["rev-list", "--first-parent", hash])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// Every commit in the history of the commit `hash`, itself included.
    pub(crate) fn ancestors(&self, hash: &str) -> Result<HashSet<String>, String> {
        let listing = self.git.git(&["rev-list", hash])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// The hash and message of each commit on the first-parent line of the
    /// commit `hash`, newest first, as `git log -z --format=%H%n%B` writes
    /// them.
    pub(crate) fn first_parent_log(&self, hash: &str) -> Result<String, String> {
        self.git
            .git(&["log", "--first-parent", "-z", "--format=%H%n%B", hash])
    }

    /// The hash, commit time, and message of each commit in the history of `to`
    /// that `from` lacks, newest first, as `git log -z --format=%H%n%ct%n%B`
    /// writes them.
    pub(crate) fn log_between(&self, from: &str, to: &str) -> Result<String, String> {
        let range = format!("{from}..{to}");

        self.git.git(&["log", "-z", "--format=%H%n%ct%n%B", &range])
    }

    /// The best common ancestor of the commits `one` and `other`.
    pub(crate) fn merge_base(&self, one: &str, other: &str) -> Result<String, String> {
        Ok(self.git.git(&["merge-base", one, other])?.trim().to_owned())
    }

    /// Whether the commits `one` and `other` hold the same files below `path`.
    pub(crate) fn same_below(&self, one: &str, other: &str, path: &str) -> Result<bool, String> {
        let ran = self
            .git
            .try_git(&["diff", "--quiet", one, other, "--", path])?;
        // It exits 1, saying nothing, when they differ.
        if ran.success || ran.stderr.trim().is_empty() {
            return Ok(ran.success);
        }

        Err(format!("`git diff` failed: {}", ran.said()))
    }

    /// Whether the commit `hash` changes, against its first parent, a path
    /// outside the directory `dir`.
    pub(crate) fn changes_outside(&self, hash: &str, dir: &str) -> Result<bool, String> {
        let changed = self.git.git(&[
            "log",
            "-1",
            "-z",
            "--format=",
            "--name-only",
            "--diff-merges=first-parent",
            hash,
        ])?;
        // `--name-only` names each path from the top of the work tree.
        let inside = format!("{}{dir}/", self.git.prefix);

        Ok(changed
            .split('\0')
            .map(|path| path.trim_start_matches('\n'))
            .any(|path| !path.is_empty() && !path.starts_with(&inside)))
    }

    /// The release branches, here or on `origin`, whose history holds the
    /// commit `hash`, by name.
    pub(crate) fn containing(&self, hash: &str) -> Result<Vec<String>, String> {
        let local = format!("{LOCAL_REFS}{RELEASE_BRANCH}");
        let origin = format!("{ORIGIN_REFS}{RELEASE_BRANCH}");
        let listing = self.git.git(&[
            "for-each-ref",
            "--format=%(refname)",
            "--contains",
            hash,
            &local,
            &origin,
        ])?;

        let mut names: Vec<String> = listing
            .lines()
            .filter_map(|reference| {
                reference
                    .strip_prefix(LOCAL_REFS)
                    .or_else(|| reference.strip_prefix(ORIGIN_REFS))
            })
            .map(ToOwned::to_owned)
            .collect();
        names.sort();
        names.dedup();

        Ok(names)
    }

    /// The files below `path`, a directory, at the commit `hash`; none when the
    /// commit has no `path`.
    pub(crate) fn files_at(&self, hash: &str, path: &str) -> Result<BTreeSet<String>, String> {
        let listing = self
            .git
            .git(&["ls-tree", "-r", "-z", "--name-only", hash, "--", path])?;

        Ok(listing
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .collect())
    }
}
