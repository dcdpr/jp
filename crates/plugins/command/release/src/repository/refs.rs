//! The branches and tags of the repository and of `origin`.

use std::{collections::BTreeMap, fs};

use super::{Branch, LOCAL_REFS, ORIGIN, ORIGIN_REFS, Tags, git::Git, temporary};
use crate::releases::{self, Standing};

/// The branches and tags of the repository and of `origin`, as last fetched.
pub(crate) struct Refs<'a> {
    git: Git<'a>,
}

impl<'a> Refs<'a> {
    pub(super) const fn new(git: Git<'a>) -> Self {
        Self { git }
    }

    /// Every branch whose name starts with `prefix`, such as `release/`, as
    /// this repository and `origin` each hold it, by name.
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

    /// The branch `name`, as this repository and `origin` each hold it.
    pub(crate) fn branch(&self, name: &str) -> Result<Branch, String> {
        Ok(Branch {
            name: name.to_owned(),
            local: self.git.hash(&format!("{LOCAL_REFS}{name}"))?,
            origin: self.git.hash(&format!("{ORIGIN_REFS}{name}"))?,
        })
    }

    /// Where `branch` in this repository stands against `origin`'s; `None` when
    /// neither has it.
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

    /// The commit `branch` is at for the release commands: this repository's
    /// when it has every commit of `origin`'s, else `origin`'s; `None` when
    /// neither has it.
    ///
    /// Refuses a branch this repository and `origin` each have commits of the
    /// other lacks.
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

    /// The `v` tags in this repository, and those on `origin`.
    pub(crate) fn tags(&self) -> Result<Tags, String> {
        // An annotated tag names a tag object, which `*objectname` peels to the
        // commit; a lightweight tag names the commit itself.
        let format = concat!(
            "--format=",
            "%(if)%(*objectname)%(then)%(*objectname)%(else)%(objectname)%(end)",
            "%09%(refname)",
        );
        let local = self.git.git(&["for-each-ref", format, "refs/tags/v*"])?;
        // Without `--refs`, each annotated tag is listed peeled too.
        let origin = self
            .git
            .git(&["ls-remote", "--tags", ORIGIN, "refs/tags/v*"])?;

        Ok(Tags {
            local: releases::parse_tags(&local),
            origin: releases::parse_tags(&origin),
        })
    }

    /// When the tag `tag` was made, in seconds since the Unix epoch: an
    /// annotated tag's own time, or a lightweight tag's commit's.
    ///
    /// Reads this repository's tag, or else `origin`'s, which is fetched into
    /// `FETCH_HEAD` alone, so no tag is written here.
    /// The error says the tag can't be read, or has no time.
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

    /// Fetch `origin`'s tag `tag` into `FETCH_HEAD`, with the objects it needs,
    /// writing no tag here.
    pub(crate) fn fetch_tag(&self, tag: &str) -> Result<(), String> {
        let reference = format!("refs/tags/{tag}");

        self.git
            .git(&["fetch", "--quiet", "--no-tags", ORIGIN, &reference])
            .map(drop)
    }

    /// The commit the tag `tag` in this repository points at, when it has one.
    pub(crate) fn local_tag(&self, tag: &str) -> Result<Option<String>, String> {
        let reference = format!("refs/tags/{tag}");
        if self.git.hash(&reference)?.is_none() {
            return Ok(None);
        }

        self.git.commit(&reference).map(Some)
    }

    /// Point this repository's branch `name` at the commit `new`.
    ///
    /// `local` is the commit the branch was at when it was read, or `None` for
    /// a branch to create; a branch that moved since is left alone.
    /// Refuses a branch checked out in a worktree, whose files would no longer
    /// match it.
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

    /// Delete this repository's branch `name`, which has to be at the commit
    /// `local` still.
    ///
    /// Refuses a branch checked out in a worktree.
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

    /// Delete this repository's tag `tag`.
    pub(crate) fn delete_tag(&self, tag: &str) -> Result<(), String> {
        self.git.git(&["tag", "--delete", tag]).map(drop)
    }

    /// Refuse when the branch `name` is checked out in one of the repository's
    /// worktrees, naming where.
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

    /// Make the annotated tag `tag` at the commit `hash`, with `message` kept
    /// as it is: lines starting with `#` stay, where git would otherwise drop
    /// them as comments.
    pub(crate) fn tag(&self, tag: &str, hash: &str, message: &str) -> Result<(), String> {
        // A file rather than an argument, since a release's notes can outgrow
        // what a command line holds on Windows.
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
