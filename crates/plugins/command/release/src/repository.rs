//! The repository holding the workspace, as `cut`, `pick`, and `finalize` reach
//! `origin` through it.
//!
//! They read what `origin` holds after fetching `main` and the release branches
//! into their remote-tracking branches, and write in worktrees of their own, so
//! the checkout they run from is left alone.
//! The default branch is `main`.

use std::{
    collections::BTreeMap,
    env, fs, process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use doc::repository::web_address;
use semver::Version;

use crate::{
    options::Bump,
    releases::{self, Line, Releases},
    shell::{Ran, Shell},
};

/// The remote releases are read from and pushed to.
pub(crate) const ORIGIN: &str = "origin";

/// The default branch, which every change reaches first.
pub(crate) const MAIN: &str = "main";

/// Where `main` is fetched to.
const MAIN_REF: &str = "refs/remotes/origin/main";

/// Where the release branches are fetched to.
const RELEASE_REFS: &str = "refs/remotes/origin/release/";

/// The repository holding a workspace.
pub(crate) struct Repository<'a> {
    shell: &'a Shell,

    /// The workspace root, where commands that concern the whole repository
    /// run.
    root: Utf8PathBuf,

    /// The path from the top of the work tree to the workspace root: empty, or
    /// ending in `/`.
    prefix: String,
}

impl<'a> Repository<'a> {
    /// The repository holding the workspace at `root`.
    ///
    /// The error says the workspace isn't in a git repository.
    pub(crate) fn open(shell: &'a Shell, root: &Utf8Path) -> Result<Self, String> {
        let ran = shell.try_git(root, &["rev-parse", "--show-prefix"])?;
        if !ran.success {
            return Err(format!(
                "The workspace at {root} isn't in a git repository: {}",
                ran.said()
            ));
        }

        Ok(Self {
            shell,
            root: root.to_owned(),
            prefix: ran.stdout.trim().to_owned(),
        })
    }

    pub(crate) fn shell(&self) -> &Shell {
        self.shell
    }

    /// The path from the top of the work tree to the workspace root: empty, or
    /// ending in `/`.
    pub(crate) fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Run git with `args` in the workspace root.
    pub(crate) fn git(&self, args: &[&str]) -> Result<String, String> {
        self.shell.git(&self.root, args)
    }

    /// Refuse a shallow clone, which lacks the history credits and shipped
    /// changes are read from.
    pub(crate) fn refuse_shallow(&self) -> Result<(), String> {
        let shallow = self.git(&["rev-parse", "--is-shallow-repository"])?;
        if shallow.trim() == "true" {
            return Err(
                "The repository is a shallow clone, which lacks the history releases are read \
                 from; fetch the rest of it with `git fetch --unshallow`."
                    .to_owned(),
            );
        }

        Ok(())
    }

    /// Fetch `main` and the release branches from `origin` into their
    /// remote-tracking branches, dropping the release branches `origin` no
    /// longer has.
    pub(crate) fn fetch(&self) -> Result<(), String> {
        let main = format!("+refs/heads/{MAIN}:{MAIN_REF}");
        let releases = format!("+refs/heads/release/*:{RELEASE_REFS}*");

        self.git(&[
            "fetch",
            "--quiet",
            "--prune",
            "--no-tags",
            ORIGIN,
            &main,
            &releases,
        ])
        .map(drop)
    }

    /// The hash of `main`, as last fetched.
    pub(crate) fn main(&self) -> Result<String, String> {
        self.commit(MAIN_REF)
    }

    /// What `origin`'s tags and release branches say, its release branches as
    /// last fetched.
    pub(crate) fn releases(&self) -> Result<Releases, String> {
        let tagged = releases::parse_tags(&self.git(&["ls-remote", "--tags", "--refs", ORIGIN])?);

        let listing = self.git(&[
            "for-each-ref",
            "--format=%(refname:lstrip=3)%00%(objectname)",
            RELEASE_REFS,
        ])?;
        let mut lines = BTreeMap::new();
        for entry in listing.lines() {
            let Some((branch, tip)) = entry.split_once('\0') else {
                continue;
            };
            let Some(key) = releases::line_of(branch) else {
                continue;
            };

            let range = format!("{MAIN_REF}..{tip}");
            let log = self.git(&["log", "-z", "--format=%H%n%ct%n%B", &range])?;
            let (started, picked) = releases::parse_line(&log)?;
            lines.insert(key, Line {
                branch: branch.to_owned(),
                tip: tip.to_owned(),
                started,
                picked,
            });
        }

        Ok(Releases { tagged, lines })
    }

    /// When `origin`'s tag `tag` was made, in seconds since the Unix epoch: an
    /// annotated tag's own time, or a lightweight tag's commit's.
    ///
    /// The error says the tag can't be fetched, or has no time.
    pub(crate) fn tag_date(&self, tag: &str) -> Result<i64, String> {
        // Into `FETCH_HEAD` alone, so no tag is written to the repository.
        let reference = format!("refs/tags/{tag}");
        self.git(&["fetch", "--quiet", "--no-tags", ORIGIN, &reference])?;

        let kind = self.git(&["cat-file", "-t", "FETCH_HEAD"])?;
        let date = if kind.trim() == "tag" {
            releases::tagger_time(&self.git(&["cat-file", "tag", "FETCH_HEAD"])?)
        } else {
            self.git(&["log", "-1", "--format=%ct", "FETCH_HEAD"])?
                .trim()
                .parse()
                .ok()
        };

        date.ok_or_else(|| format!("The tag {tag} on {ORIGIN} has no time git can read."))
    }

    /// The web address of `origin`, when it has one.
    pub(crate) fn origin_address(&self) -> Option<String> {
        let url = self.git(&["remote", "get-url", ORIGIN]).ok()?;

        web_address(url.trim())
    }

    /// The full hash of the commit `revision` names.
    ///
    /// The error says it names no commit.
    pub(crate) fn commit(&self, revision: &str) -> Result<String, String> {
        let peeled = format!("{revision}^{{commit}}");
        let ran = self
            .shell
            .try_git(&self.root, &["rev-parse", "--verify", "--quiet", &peeled])?;
        if !ran.success {
            return Err(format!("`{revision}` names no commit."));
        }

        Ok(ran.stdout.trim().to_owned())
    }

    /// The first-parent line of `main`, as last fetched, newest first.
    pub(crate) fn main_line(&self) -> Result<Vec<String>, String> {
        let listing = self.git(&["rev-list", "--first-parent", MAIN_REF])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// The release branches whose history holds the commit `hash`, by name.
    pub(crate) fn containing(&self, hash: &str) -> Result<Vec<String>, String> {
        let listing = self.git(&[
            "for-each-ref",
            "--format=%(refname:lstrip=3)",
            "--contains",
            hash,
            RELEASE_REFS,
        ])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// Whether `origin` has a branch named `branch`.
    pub(crate) fn has_branch(&self, branch: &str) -> Result<bool, String> {
        let reference = format!("refs/heads/{branch}");
        let listing = self.git(&["ls-remote", "--heads", ORIGIN, &reference])?;

        Ok(listing.lines().any(|line| {
            line.split_once('\t')
                .is_some_and(|(_, name)| name == reference)
        }))
    }

    /// The git directory the repository's worktrees share.
    pub(crate) fn common_dir(&self) -> Result<Utf8PathBuf, String> {
        let dir = Utf8PathBuf::from(self.git(&["rev-parse", "--git-common-dir"])?.trim());

        Ok(if dir.is_absolute() {
            dir
        } else {
            self.root.join(dir)
        })
    }

    /// A new worktree at the commit `hash`, named after `purpose`, which is
    /// removed when dropped.
    pub(crate) fn worktree(&self, purpose: &str, hash: &str) -> Result<Worktree<'_>, String> {
        let top = temporary(purpose)?;
        self.git(&["worktree", "add", "--detach", "--quiet", top.as_str(), hash])?;

        Ok(Worktree {
            repo: self,
            top,
            kept: false,
        })
    }

    /// The worktree at `top` a stopped run kept, to finish it.
    ///
    /// The error says there is no worktree there.
    pub(crate) fn reopen(&self, top: &Utf8Path) -> Result<Worktree<'_>, String> {
        if !top.join(".git").exists() {
            return Err(format!("There is no worktree at {top}."));
        }

        Ok(Worktree {
            repo: self,
            top: top.to_owned(),
            kept: false,
        })
    }

    /// Push the commit `hash` to `origin` as the branch `branch`, with each of
    /// `options` as a push option.
    ///
    /// With `create`, the push refuses to replace a branch `origin` has.
    /// Returns what git printed, which holds what the remote said.
    pub(crate) fn push(
        &self,
        hash: &str,
        branch: &str,
        create: bool,
        options: &[String],
    ) -> Result<String, String> {
        let reference = format!("refs/heads/{branch}");
        let lease = format!("--force-with-lease={reference}:");
        let refspec = format!("{hash}:{reference}");

        let mut args = vec!["push"];
        if create {
            args.push(&lease);
        }
        for option in options {
            args.extend(["--push-option", option]);
        }
        args.extend([ORIGIN, &refspec]);

        let ran = self.shell.try_git(&self.root, &args)?;
        if !ran.success {
            return Err(format!(
                "Pushing {branch} to {ORIGIN} failed: {}",
                ran.said()
            ));
        }

        Ok(ran.stderr)
    }

    /// Push the tag `tag` to `origin`, which refuses to replace a tag it has.
    pub(crate) fn push_tag(&self, tag: &str) -> Result<(), String> {
        let refspec = format!("refs/tags/{tag}:refs/tags/{tag}");
        let ran = self
            .shell
            .try_git(&self.root, &["push", ORIGIN, &refspec])?;
        if !ran.success {
            return Err(format!("Pushing {tag} to {ORIGIN} failed: {}", ran.said()));
        }

        Ok(())
    }
}

/// A worktree of the repository, removed when dropped unless kept.
pub(crate) struct Worktree<'r> {
    repo: &'r Repository<'r>,

    /// The top of the worktree.
    top: Utf8PathBuf,

    kept: bool,
}

impl Worktree<'_> {
    /// The top of the worktree.
    pub(crate) fn top(&self) -> &Utf8Path {
        &self.top
    }

    /// The workspace root in this worktree.
    pub(crate) fn workspace(&self) -> Utf8PathBuf {
        self.top.join(self.repo.prefix())
    }

    /// Run git with `args` in the workspace root in this worktree.
    pub(crate) fn git(&self, args: &[&str]) -> Result<String, String> {
        self.repo.shell().git(&self.workspace(), args)
    }

    /// Run git with `args` in the workspace root in this worktree, whatever it
    /// exits with.
    pub(crate) fn try_git(&self, args: &[&str]) -> Result<Ran, String> {
        self.repo.shell().try_git(&self.workspace(), args)
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
    /// The error quotes what it printed.
    pub(crate) fn bump(&self, bump: &Bump, version: &Version) -> Result<(), String> {
        let (program, args) = bump.command(version);
        let ran = self.repo.shell().run(&self.workspace(), &program, &args)?;
        if !ran.success {
            return Err(format!("`bump` failed writing {version}: {}", ran.said()));
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
            self.repo
                .git(&["worktree", "remove", "--force", self.top.as_str()]),
        );
        if self.top.exists() {
            drop(fs::remove_dir_all(&self.top));
            drop(self.repo.git(&["worktree", "prune"]));
        }
    }
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
