//! Reading and updating local and `origin` branches and tags.

use std::{collections::BTreeMap, fs};

use super::{Branch, LOCAL_REFS, ORIGIN, ORIGIN_REFS, Tags, git::Git, temporary};
use crate::releases::{self, Standing};

pub(crate) struct Refs<'a> {
    git: Git<'a>,
}

impl<'a> Refs<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    /// List all branches matching `prefix` (e.g. `"release/"`) across
    /// `refs/heads/` and `refs/remotes/origin/`.
    pub(crate) fn branches(&self, prefix: &str) -> Result<Vec<Branch>, String> {
        let mut branches: BTreeMap<String, Branch> = BTreeMap::new();
        for (namespace, local) in [(LOCAL_REFS, true), (ORIGIN_REFS, false)] {
            let pattern = format!("{namespace}{prefix}");
            let listing = self.git.git(&[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)",
                &pattern,
            ])?;
            for (name, hash) in listing
                .lines()
                .filter_map(|entry| entry.split_once('\0'))
                .filter_map(|(reference, hash)| Some((reference.strip_prefix(namespace)?, hash)))
            {
                let branch = branches.entry(name.to_owned()).or_insert_with(|| Branch {
                    name: name.to_owned(),
                    local: None,
                    origin: None,
                });
                let side = if local {
                    &mut branch.local
                } else {
                    &mut branch.origin
                };
                *side = Some(hash.to_owned());
            }
        }

        Ok(branches.into_values().collect())
    }

    pub(crate) fn branch(&self, name: &str) -> Result<Branch, String> {
        Ok(Branch {
            name: name.to_owned(),
            local: self.git.hash(&format!("{LOCAL_REFS}{name}"))?,
            origin: self.git.hash(&format!("{ORIGIN_REFS}{name}"))?,
        })
    }

    pub(crate) fn standing(&self, branch: &Branch) -> Result<Option<Standing>, String> {
        let standing = match (&branch.local, &branch.origin) {
            (None, None) => return Ok(None),
            (Some(_), None) => Standing::Local,
            (None, Some(_)) => Standing::Remote,
            (Some(local), Some(origin)) if local == origin => Standing::Published,
            (Some(local), Some(origin)) if self.git.is_ancestor(origin, local)? => Standing::Ahead,
            (Some(local), Some(origin)) if self.git.is_ancestor(local, origin)? => Standing::Behind,
            (Some(_), Some(_)) => Standing::Diverged,
        };

        Ok(Some(standing))
    }

    /// Return the further-ahead tip commit between local and `origin` for
    /// `branch`, or an error if the two have diverged.
    pub(crate) fn tip(&self, branch: &Branch) -> Result<Option<String>, String> {
        Ok(match self.standing(branch)? {
            None => None,
            Some(Standing::Local | Standing::Published | Standing::Ahead) => branch.local.clone(),
            Some(Standing::Remote | Standing::Behind) => branch.origin.clone(),
            Some(Standing::Diverged) => {
                return Err(format!(
                    "{name} and {ORIGIN}/{name} have diverged; bring them together, such as with \
                     `git rebase {ORIGIN}/{name} {name}`, and run the command again.",
                    name = branch.name
                ));
            }
        })
    }

    /// List all `v*` tags locally and on `origin`.
    pub(crate) fn tags(&self) -> Result<Tags, String> {
        let format = concat!(
            "--format=",
            "%(if)%(*objectname)%(then)%(*objectname)%(else)%(objectname)%(end)",
            "%09%(refname)",
        );
        let local = self.git.git(&["for-each-ref", format, "refs/tags/v*"])?;
        let origin = self
            .git
            .git(&["ls-remote", "--tags", ORIGIN, "refs/tags/v*"])?;

        Ok(Tags {
            local: releases::parse_tags(&local),
            origin: releases::parse_tags(&origin),
        })
    }

    /// Return the Unix timestamp of `tag` (the tagger date for annotated tags
    /// or the commit date for lightweight tags), fetching into `FETCH_HEAD` if
    /// the tag only exists on `origin`.
    pub(crate) fn tag_date(&self, tag: &str) -> Result<i64, String> {
        let reference = format!("refs/tags/{tag}");
        let object = if self.git.hash(&reference)?.is_some() {
            reference
        } else {
            self.fetch_tag(tag)?;
            "FETCH_HEAD".to_owned()
        };

        let kind = self.git.git(&["cat-file", "-t", &object])?;
        let date = if kind.trim() == "tag" {
            releases::tagger_time(&self.git.git(&["cat-file", "tag", &object])?)
        } else {
            self.git
                .git(&["log", "-1", "--format=%ct", &object])?
                .trim()
                .parse()
                .ok()
        };

        date.ok_or_else(|| format!("The tag {tag} has no time git can read."))
    }

    /// Fetch `refs/tags/<tag>` from `origin` into `FETCH_HEAD` without creating
    /// a local tag ref.
    pub(crate) fn fetch_tag(&self, tag: &str) -> Result<(), String> {
        let reference = format!("refs/tags/{tag}");

        self.git
            .git(&["fetch", "--quiet", "--no-tags", ORIGIN, &reference])
            .map(drop)
    }

    pub(crate) fn local_tag(&self, tag: &str) -> Result<Option<String>, String> {
        let reference = format!("refs/tags/{tag}");
        if self.git.hash(&reference)?.is_none() {
            return Ok(None);
        }

        self.git.commit(&reference).map(Some)
    }

    /// Atomically update `refs/heads/<name>` from `local` (`None` if creating a
    /// new branch) to `new`, rejecting the update if the branch is checked out
    /// in any worktree.
    pub(crate) fn set_branch(
        &self,
        name: &str,
        new: &str,
        local: Option<&str>,
    ) -> Result<(), String> {
        self.refuse_checked_out(name)?;

        let reference = format!("{LOCAL_REFS}{name}");
        let expected = local.unwrap_or_default();
        let ran =
            self.git
                .try_git(&["update-ref", "-m", "jp release", &reference, new, expected])?;
        if !ran.success {
            return Err(format!(
                "{name} moved while the command ran, so it wasn't changed: {}",
                ran.said()
            ));
        }

        Ok(())
    }

    /// Atomically delete `refs/heads/<name>` (expecting it at commit `local`),
    /// rejecting the deletion if the branch is checked out in any worktree.
    pub(crate) fn delete_branch(&self, name: &str, local: &str) -> Result<(), String> {
        self.refuse_checked_out(name)?;

        let reference = format!("{LOCAL_REFS}{name}");
        let ran = self
            .git
            .try_git(&["update-ref", "-m", "jp release", "-d", &reference, local])?;
        if !ran.success {
            return Err(format!(
                "{name} moved while the command ran, so it wasn't deleted: {}",
                ran.said()
            ));
        }

        Ok(())
    }

    pub(crate) fn delete_tag(&self, tag: &str) -> Result<(), String> {
        self.git.git(&["tag", "--delete", tag]).map(drop)
    }

    /// Return an error if branch `name` is currently checked out in any
    /// worktree.
    pub(crate) fn refuse_checked_out(&self, name: &str) -> Result<(), String> {
        let listing = self.git.git(&["worktree", "list", "--porcelain"])?;
        let reference = format!("branch {LOCAL_REFS}{name}");

        let mut worktree = "";
        for line in listing.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                worktree = path;
            } else if line == reference {
                return Err(format!(
                    "{name} is checked out at {worktree}; switch that worktree to another branch \
                     and run the command again."
                ));
            }
        }

        Ok(())
    }

    /// Create an annotated tag `tag` at `hash` with verbatim `message`
    /// (`--cleanup=verbatim` preserves Markdown `#` headings).
    pub(crate) fn tag(&self, tag: &str, hash: &str, message: &str) -> Result<(), String> {
        let file = temporary("tag-message")?;
        fs::write(&file, message)
            .map_err(|error| format!("failed to write the message of {tag} to {file}: {error}"))?;
        let made = self.git.git(&[
            "tag",
            "--annotate",
            "--cleanup=verbatim",
            "--file",
            file.as_str(),
            tag,
            hash,
        ]);
        drop(fs::remove_file(&file));

        made.map(drop)
    }
}
