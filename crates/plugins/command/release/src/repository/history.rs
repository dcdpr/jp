//! Commit history and tree queries on [`super::Repository`].

use std::collections::{BTreeSet, HashSet};

use super::{LOCAL_REFS, ORIGIN_REFS, git::Git};
use crate::releases::RELEASE_BRANCH;

/// Commit history and tree queries, with all paths resolved relative to the
/// workspace root.
pub(crate) struct History<'a> {
    git: Git<'a>,
}

impl<'a> History<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    pub(crate) fn commit(&self, revision: &str) -> Result<String, String> {
        self.git.commit(revision)
    }

    pub(crate) fn has_commit(&self, hash: &str) -> Result<bool, String> {
        let object = format!("{hash}^{{commit}}");

        Ok(self.git.hash(&object)?.is_some())
    }

    pub(crate) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        self.git.is_ancestor(ancestor, descendant)
    }

    pub(crate) fn subject(&self, hash: &str) -> Result<String, String> {
        let subject = self.git.git(&["log", "-1", "--format=%s", hash])?;

        Ok(subject.trim().to_owned())
    }

    /// Return the first-parent commit hashes of `hash`, newest first.
    pub(crate) fn first_parents(&self, hash: &str) -> Result<Vec<String>, String> {
        let listing = self.git.git(&["rev-list", "--first-parent", hash])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// Return all ancestor commit hashes of `hash` (including `hash` itself).
    pub(crate) fn ancestors(&self, hash: &str) -> Result<HashSet<String>, String> {
        let listing = self.git.git(&["rev-list", hash])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// Return `git log --first-parent -z --format=%H%n%B` for `hash`.
    pub(crate) fn first_parent_log(&self, hash: &str) -> Result<String, String> {
        self.git
            .git(&["log", "--first-parent", "-z", "--format=%H%n%B", hash])
    }

    /// Return `git log -z --format=%H%n%ct%n%B` for `from..to`.
    pub(crate) fn log_between(&self, from: &str, to: &str) -> Result<String, String> {
        let range = format!("{from}..{to}");

        self.git.git(&["log", "-z", "--format=%H%n%ct%n%B", &range])
    }

    pub(crate) fn merge_base(&self, one: &str, other: &str) -> Result<String, String> {
        Ok(self.git.git(&["merge-base", one, other])?.trim().to_owned())
    }

    /// Return whether commits `one` and `other` have identical trees under
    /// `path`.
    pub(crate) fn same_below(&self, one: &str, other: &str, path: &str) -> Result<bool, String> {
        let ran = self
            .git
            .try_git(&["diff", "--quiet", one, other, "--", path])?;
        if ran.success || ran.stderr.trim().is_empty() {
            return Ok(ran.success);
        }

        Err(format!("`git diff` failed: {}", ran.said()))
    }

    /// Return whether `hash` (diffed against its first parent) modifies any
    /// path outside `dir`.
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
        let inside = format!("{}{dir}/", self.git.prefix);

        Ok(changed
            .split('\0')
            .map(|path| path.trim_start_matches('\n'))
            .any(|path| !path.is_empty() && !path.starts_with(&inside)))
    }

    /// Return the names of all local and `origin` release branches whose
    /// history contains `hash`.
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

    /// Return all file paths under `path` at commit `hash`.
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
