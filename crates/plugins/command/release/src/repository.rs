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

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    env, fs, process,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use doc::repository::web_address;
use semver::Version;

use crate::{
    options::{BUMP_KEY, Bump},
    pipeline::short,
    releases::{self, Line, NOTES_BRANCH, RELEASE_BRANCH, Releases, Standing},
    shell::{Ran, Shell},
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
    shell: &'a Shell,

    /// The workspace root, where commands that concern the whole repository
    /// run.
    root: Utf8PathBuf,

    /// The path from the top of the work tree to the workspace root: empty, or
    /// ending in `/`.
    prefix: String,
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
    /// them in.
    pub(crate) fn behind(&self) -> String {
        format!(
            "{} lacks commits {ORIGIN}'s {MAIN} has, at {}; bring them in first, such as with \
             `git pull`.",
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

    /// The workspace root, in the checkout the command runs from.
    pub(crate) fn root(&self) -> &Utf8Path {
        &self.root
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

    /// Fetch `main`, the release branches, and the notes branches from `origin`
    /// into their remote-tracking branches, dropping the branches `origin` no
    /// longer has.
    pub(crate) fn fetch(&self) -> Result<(), String> {
        let main = format!("+refs/heads/{MAIN}:{MAIN_REF}");
        let releases = format!("+refs/heads/{RELEASE_BRANCH}*:{ORIGIN_REFS}{RELEASE_BRANCH}*");
        let notes = format!("+refs/heads/{NOTES_BRANCH}*:{ORIGIN_REFS}{NOTES_BRANCH}*");

        self.git(&[
            "fetch",
            "--quiet",
            "--prune",
            "--no-tags",
            ORIGIN,
            &main,
            &releases,
            &notes,
        ])
        .map(drop)
    }

    /// The hash of `origin`'s `main`, as last fetched.
    pub(crate) fn main(&self) -> Result<String, String> {
        self.commit(MAIN_REF)
    }

    /// The checkout's commit, which stands for `main`.
    ///
    /// Refuses a checkout without the commits of `origin`'s `main`, as last
    /// fetched: what is made from it would leave out work `main` has.
    pub(crate) fn checkout(&self) -> Result<Checkout, String> {
        let checkout = self.checked_out()?;
        if self.lacks_main(&checkout)? {
            return Err(checkout.behind());
        }

        Ok(checkout)
    }

    /// The checkout's commit, whether or not it has the commits of `origin`'s
    /// `main`.
    pub(crate) fn checked_out(&self) -> Result<Checkout, String> {
        let head = self.commit("HEAD").map_err(|_| {
            "The checkout has no commit yet, so there is nothing to release.".to_owned()
        })?;
        let branch =
            self.shell
                .try_git(&self.root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let branch = branch.success.then(|| branch.stdout.trim().to_owned());
        let main = self.main()?;

        Ok(Checkout { head, branch, main })
    }

    /// Whether `checkout` lacks commits of `origin`'s `main`, as last fetched.
    pub(crate) fn lacks_main(&self, checkout: &Checkout) -> Result<bool, String> {
        Ok(!self.is_ancestor(&checkout.main, &checkout.head)?)
    }

    /// Whether the commit `ancestor` is in the history of `descendant`, or is
    /// it.
    pub(crate) fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let ran = self.shell.try_git(&self.root, &[
            "merge-base",
            "--is-ancestor",
            ancestor,
            descendant,
        ])?;
        // It exits 1, saying nothing, for a commit that isn't an ancestor.
        if ran.success || ran.stderr.trim().is_empty() {
            return Ok(ran.success);
        }

        Err(format!("`git merge-base` failed: {}", ran.said()))
    }

    /// Every branch whose name starts with `prefix`, such as `release/`, as
    /// this repository and `origin` each hold it, by name.
    pub(crate) fn branches(&self, prefix: &str) -> Result<Vec<Branch>, String> {
        let mut branches: BTreeMap<String, Branch> = BTreeMap::new();
        let mut list = |namespace: &str, local: bool| {
            let pattern = format!("{namespace}{prefix}");
            let listing = self.git(&[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)",
                &pattern,
            ])?;
            for entry in listing.lines() {
                let Some((reference, hash)) = entry.split_once('\0') else {
                    continue;
                };
                let Some(name) = reference.strip_prefix(namespace) else {
                    continue;
                };
                let branch = branches.entry(name.to_owned()).or_insert_with(|| Branch {
                    name: name.to_owned(),
                    local: None,
                    origin: None,
                });
                if local {
                    branch.local = Some(hash.to_owned());
                } else {
                    branch.origin = Some(hash.to_owned());
                }
            }

            Ok::<_, String>(())
        };
        list(LOCAL_REFS, true)?;
        list(ORIGIN_REFS, false)?;

        Ok(branches.into_values().collect())
    }

    /// The branch `name`, as this repository and `origin` each hold it.
    pub(crate) fn branch(&self, name: &str) -> Result<Branch, String> {
        Ok(Branch {
            name: name.to_owned(),
            local: self.hash(&format!("{LOCAL_REFS}{name}"))?,
            origin: self.hash(&format!("{ORIGIN_REFS}{name}"))?,
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
            (Some(local), Some(origin)) if self.is_ancestor(origin, local)? => Standing::Ahead,
            (Some(local), Some(origin)) if self.is_ancestor(local, origin)? => Standing::Behind,
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
        let local = self.git(&["for-each-ref", format, "refs/tags/v*"])?;
        // Without `--refs`, each annotated tag is listed peeled too.
        let origin = self.git(&["ls-remote", "--tags", ORIGIN, "refs/tags/v*"])?;

        Ok(Tags {
            local: releases::parse_tags(&local),
            origin: releases::parse_tags(&origin),
        })
    }

    /// What the tags and release branches here and on `origin` say, each
    /// release branch at its [`tip`].
    ///
    /// [`tip`]: Self::tip
    pub(crate) fn releases(&self) -> Result<Releases, String> {
        self.releases_and_tags().map(|(releases, _)| releases)
    }

    /// [`releases`], and the tags they were read from, for a command that needs
    /// both without asking `origin` for the tags twice.
    ///
    /// [`releases`]: Self::releases
    pub(crate) fn releases_and_tags(&self) -> Result<(Releases, Tags), String> {
        let tags = self.tags()?;
        let published = releases::versions(tags.origin.keys());
        let tagged = releases::versions(tags.local.keys())
            .union(&published)
            .cloned()
            .collect();

        let main = self.main()?;
        let mut lines = BTreeMap::new();
        for branch in self.branches(RELEASE_BRANCH)? {
            let Some(key) = releases::line_of(&branch.name) else {
                continue;
            };
            let Some(tip) = self.tip(&branch)? else {
                continue;
            };

            let range = format!("{main}..{tip}");
            let log = self.git(&["log", "-z", "--format=%H%n%ct%n%B", &range])?;
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

    /// When the tag `tag` was made, in seconds since the Unix epoch: an
    /// annotated tag's own time, or a lightweight tag's commit's.
    ///
    /// Reads this repository's tag, or else `origin`'s, which is fetched into
    /// `FETCH_HEAD` alone, so no tag is written here.
    /// The error says the tag can't be read, or has no time.
    pub(crate) fn tag_date(&self, tag: &str) -> Result<i64, String> {
        let reference = format!("refs/tags/{tag}");
        let object = if self.hash(&reference)?.is_some() {
            reference
        } else {
            self.fetch_tag(tag)?;
            "FETCH_HEAD".to_owned()
        };

        let kind = self.git(&["cat-file", "-t", &object])?;
        let date = if kind.trim() == "tag" {
            releases::tagger_time(&self.git(&["cat-file", "tag", &object])?)
        } else {
            self.git(&["log", "-1", "--format=%ct", &object])?
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

        self.git(&["fetch", "--quiet", "--no-tags", ORIGIN, &reference])
            .map(drop)
    }

    /// Whether this repository has the commit `hash`.
    pub(crate) fn has_commit(&self, hash: &str) -> Result<bool, String> {
        let object = format!("{hash}^{{commit}}");

        Ok(self.hash(&object)?.is_some())
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

    /// The hash of the object the reference `reference` names, when it names
    /// one.
    fn hash(&self, reference: &str) -> Result<Option<String>, String> {
        let ran =
            self.shell
                .try_git(&self.root, &["rev-parse", "--verify", "--quiet", reference])?;

        Ok(ran.success.then(|| ran.stdout.trim().to_owned()))
    }

    /// The commit the tag `tag` in this repository points at, when it has one.
    pub(crate) fn local_tag(&self, tag: &str) -> Result<Option<String>, String> {
        let reference = format!("refs/tags/{tag}");
        if self.hash(&reference)?.is_none() {
            return Ok(None);
        }

        self.commit(&reference).map(Some)
    }

    /// The first-parent line of the commit `hash`, newest first.
    pub(crate) fn first_parents(&self, hash: &str) -> Result<Vec<String>, String> {
        let listing = self.git(&["rev-list", "--first-parent", hash])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// Every commit in the history of the commit `hash`, itself included.
    pub(crate) fn ancestors(&self, hash: &str) -> Result<HashSet<String>, String> {
        let listing = self.git(&["rev-list", hash])?;

        Ok(listing.lines().map(ToOwned::to_owned).collect())
    }

    /// The hash and message of each commit on the first-parent line of the
    /// commit `hash`, newest first, as `git log -z --format=%H%n%B` writes
    /// them.
    pub(crate) fn first_parent_log(&self, hash: &str) -> Result<String, String> {
        self.git(&["log", "--first-parent", "-z", "--format=%H%n%B", hash])
    }

    /// The best common ancestor of the commits `one` and `other`.
    pub(crate) fn merge_base(&self, one: &str, other: &str) -> Result<String, String> {
        Ok(self.git(&["merge-base", one, other])?.trim().to_owned())
    }

    /// Whether the commits `one` and `other` hold the same files below `path`,
    /// a path relative to the workspace root.
    pub(crate) fn same_below(&self, one: &str, other: &str, path: &str) -> Result<bool, String> {
        let ran = self
            .shell
            .try_git(&self.root, &["diff", "--quiet", one, other, "--", path])?;
        // It exits 1, saying nothing, when they differ.
        if ran.success || ran.stderr.trim().is_empty() {
            return Ok(ran.success);
        }

        Err(format!("`git diff` failed: {}", ran.said()))
    }

    /// The release branches, here or on `origin`, whose history holds the
    /// commit `hash`, by name.
    pub(crate) fn containing(&self, hash: &str) -> Result<Vec<String>, String> {
        let local = format!("{LOCAL_REFS}{RELEASE_BRANCH}");
        let origin = format!("{ORIGIN_REFS}{RELEASE_BRANCH}");
        let listing = self.git(&[
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
        let ran = self.shell.try_git(&self.root, &[
            "update-ref",
            "-m",
            "jp release",
            &reference,
            new,
            expected,
        ])?;
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
        let ran = self.shell.try_git(&self.root, &[
            "update-ref",
            "-m",
            "jp release",
            "-d",
            &reference,
            local,
        ])?;
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
        self.git(&["tag", "--delete", tag]).map(drop)
    }

    /// Refuse when the branch `name` is checked out in one of the repository's
    /// worktrees, naming where.
    pub(crate) fn refuse_checked_out(&self, name: &str) -> Result<(), String> {
        let listing = self.git(&["worktree", "list", "--porcelain"])?;
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
        let made = self.git(&[
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

    /// The files below `path`, a directory relative to the workspace root, at
    /// the commit `hash`, each relative to the workspace root; none when the
    /// commit has no `path`.
    pub(crate) fn files_at(&self, hash: &str, path: &str) -> Result<BTreeSet<String>, String> {
        let listing = self.git(&["ls-tree", "-r", "-z", "--name-only", hash, "--", path])?;

        Ok(listing
            .split('\0')
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .collect())
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
    /// The error names the setting `bump` comes from, and quotes what it
    /// printed.
    pub(crate) fn bump(&self, bump: &Bump, version: &Version) -> Result<(), String> {
        let (program, args) = bump.command(version);
        let ran = self
            .repo
            .shell()
            .run(&self.workspace(), &program, &args)
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
