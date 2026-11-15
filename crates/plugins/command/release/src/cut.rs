//! `jp release cut`: starting the next minor or major release from `main`.
//!
//! Creates two local branches without pushing:
//!
//! - `release/<major>.<minor>`, starting at the current checkout (or an earlier
//!   first-parent commit passed via `--at`) with a commit that removes the
//!   fragments directory and runs `bump`.
//! - `release-notes/v<version>`, starting at the current checkout with a commit
//!   that moves all unreleased fragments and highlights shipped by the release
//!   branch into `<fragments_dir>/<version>/` and runs `bump`.
//!   Merging this branch into `main` files the release notes.
//!
//! A fragment or highlight is attributed to the commits referenced by its
//! `Credits` fields, or to the commit that added it when `Credits` is omitted.
//! A top-level file is considered unreleased unless an existing release branch
//! already contains all of its attributed commits.
//!
//! Refuses to run while any local-only release branch or `v<version>` tag
//! exists, so leftover rehearsal refs cannot be mistaken for published
//! releases.

use std::{
    collections::{HashMap, hash_map::Entry},
    fs,
    num::NonZeroU64,
};

use changelog::{
    Forge, History, ListedCredit, Place,
    directory::{self, Contents, Part, ReadError},
};
use semver::Version;

use crate::{
    pipeline::{Pipeline, files, listed, problems, push, short},
    releases::{self, RELEASE_BRANCH, Releases},
    repository::{Checkout, MAIN, ORIGIN, Repository, Worktree},
};

/// A fragment or highlight at the top level of the fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopLevel {
    /// Path relative to the fragments directory.
    pub path: String,
    pub highlight: bool,
    pub breaking: bool,

    /// Commit hashes of the changes described by this file (from `Credits` or
    /// the commit that added the file).
    pub changes: Vec<String>,

    /// Release branches that contain every commit in `changes`.
    /// Empty when the file is unreleased.
    pub shipping: Vec<String>,
}

/// Cut the next minor or major release (or `requested`) from the current
/// checkout (or the earlier commit `at`) and return a summary with the `git
/// push` command to publish both branches.
pub(crate) fn cut(
    pipeline: &Pipeline<'_>,
    requested: Option<&Version>,
    at: Option<&str>,
) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.origin.refuse_shallow()?;
    repo.origin.fetch()?;
    let checkout = repo.origin.checkout()?;
    refuse_unpublished(repo)?;
    let releases = repo.releases()?;
    let at = cut_point(repo, &checkout, at)?;

    let notes = repo.worktrees.add("cut", &checkout.head)?;
    let filed = shipped_at(pipeline, &releases, &notes, &checkout, &at)?;
    let version = next_version(pipeline, &releases, &filed, requested)?;
    let cut = Cut::new(pipeline, &checkout, &at, version, &filed);
    refuse_existing(repo, &cut.notes_branch)?;

    let noted = file_notes(
        pipeline,
        &notes,
        &filed,
        &cut.version,
        &cut.message(pipeline),
    )?;
    let started = start_branch(pipeline, &at, &cut.version)?;
    repo.refs.set_branch(&cut.branch, &started, None)?;
    if let Err(error) = repo.refs.set_branch(&cut.notes_branch, &noted, None) {
        drop(repo.refs.delete_branch(&cut.branch, &started));
        return Err(error);
    }

    cut.report(repo, &checkout, &at)
}

struct Cut {
    version: Version,
    branch: String,
    notes_branch: String,
    from: String,
    files: String,
    destination: String,
}

impl Cut {
    fn new(
        pipeline: &Pipeline<'_>,
        checkout: &Checkout,
        at: &str,
        version: Version,
        filed: &[TopLevel],
    ) -> Self {
        let from = if at == checkout.head {
            checkout.describe()
        } else {
            format!(
                "{}, an earlier commit of {}",
                short(at),
                checkout.describe()
            )
        };
        let fragments = filed.iter().filter(|file| !file.highlight).count();

        Self {
            branch: releases::branch(&version),
            notes_branch: releases::notes_branch(&version),
            from,
            files: files(fragments, filed.len() - fragments),
            destination: format!("{}/{version}/", pipeline.fragments),
            version,
        }
    }

    fn message(&self, pipeline: &Pipeline<'_>) -> String {
        let bump = if pipeline.options.bump.is_some() {
            format!(", and writes the version {}", self.version)
        } else {
            String::new()
        };

        format!(
            "chore(release): Cut release {version}\n\n`jp release cut` started release {version} \
             on `{}`, from {}.\n\nThis moves the {} it ships into `{}`, where its notes are \
             prepared{bump}.\n",
            self.branch,
            self.from,
            self.files,
            self.destination,
            version = self.version,
        )
    }

    fn report(
        &self,
        repo: &Repository<'_>,
        checkout: &Checkout,
        at: &str,
    ) -> Result<String, String> {
        let (branch, notes_branch) = (&self.branch, &self.notes_branch);

        let mut said = format!(
            "Started release {} on {branch}, cut from {}.\n",
            self.version, self.from
        );
        if let Some(ahead) = checkout.ahead() {
            let mut holding = vec![];
            if !repo.history.is_ancestor(at, &checkout.main)? {
                holding.push(branch.clone());
            }
            holding.push(notes_branch.clone());
            said.push_str(&format!(
                "{ahead}: {} {} commits {MAIN} doesn't have yet.\n",
                listed(&holding),
                if holding.len() == 1 { "holds" } else { "hold" }
            ));
        }
        said.push_str(&format!(
            "Filed {} into {} on {notes_branch}.\n\nNothing is published yet. Publish both \
             branches with:\n\n{}\nthen prepare the release's notes on {notes_branch}, and merge \
             it into {MAIN} once they are final.\n",
            self.files,
            self.destination,
            push(&[branch.clone(), notes_branch.clone()])
        ));

        Ok(said)
    }
}

/// Return the unreleased top-level fragments and highlights in `notes` whose
/// changes are ancestors of `at`.
fn shipped_at(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    notes: &Worktree<'_>,
    checkout: &Checkout,
    at: &str,
) -> Result<Vec<TopLevel>, String> {
    let unreleased = unreleased(pipeline, releases, notes)?;
    if !unreleased.iter().any(|file| !file.highlight) {
        return Err(format!(
            "{} has no unreleased fragments, so there is nothing to release.",
            checkout.describe()
        ));
    }

    let shipped = pipeline.repo.history.ancestors(at)?;
    let filed: Vec<TopLevel> = unreleased
        .into_iter()
        .filter(|file| file.changes.iter().all(|change| shipped.contains(change)))
        .collect();
    if !filed.iter().any(|file| !file.highlight) {
        return Err(format!(
            "None of the unreleased fragments on {} describes a change {} has, so a release cut \
             there would list nothing; write fragments whose `Credits` name its changes first.",
            checkout.describe(),
            short(at)
        ));
    }

    Ok(filed)
}

fn refuse_existing(repo: &Repository<'_>, notes_branch: &str) -> Result<(), String> {
    let existing = repo.refs.branch(notes_branch)?;
    if existing.local.is_none() && existing.origin.is_none() {
        return Ok(());
    }

    Err(format!(
        "{notes_branch} exists already; delete it if an earlier cut left it behind, here with \
         `git branch -D {notes_branch}` or on {ORIGIN} with `git push {ORIGIN} --delete \
         {notes_branch}`."
    ))
}

fn next_version(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    filed: &[TopLevel],
    requested: Option<&Version>,
) -> Result<Version, String> {
    let breaking: Vec<&str> = filed
        .iter()
        .filter(|file| file.breaking)
        .map(|file| file.path.as_str())
        .collect();
    let version = releases.next_cut(!breaking.is_empty(), requested)?;
    let Some(first) = breaking.first() else {
        return Ok(version);
    };
    if releases::can_break(&version) {
        return Ok(version);
    }

    let which = match breaking.len() {
        1 => format!("{}/{first} has an upgrade section", pipeline.fragments),
        count => format!(
            "{}/{first} and {} more have upgrade sections",
            pipeline.fragments,
            count - 1
        ),
    };
    Err(format!(
        "{version} is a minor release from 1.0 on, which can't ship a change that requires users \
         to act, and {which}. Leave out `--version` to cut the next major release."
    ))
}

fn cut_point(
    repo: &Repository<'_>,
    checkout: &Checkout,
    at: Option<&str>,
) -> Result<String, String> {
    let Some(at) = at else {
        return Ok(checkout.head.clone());
    };
    let hash = repo.history.commit(at)?;
    if hash == checkout.head {
        return Ok(hash);
    }

    if !repo.history.first_parents(&checkout.head)?.contains(&hash) {
        return Err(format!(
            "{} isn't on the first-parent line of {}, so it isn't a commit of {MAIN}'s own \
             history; a release is cut at one of those.",
            short(&hash),
            checkout.describe()
        ));
    }
    let shipping = repo.history.containing(&hash)?;
    if !shipping.is_empty() {
        return Err(format!(
            "{} {} {} already, so a release cut there would ship nothing new; name a later commit.",
            listed(&shipping),
            if shipping.len() == 1 { "ships" } else { "ship" },
            short(&hash)
        ));
    }

    Ok(hash)
}

fn refuse_unpublished(repo: &Repository<'_>) -> Result<(), String> {
    let branches: Vec<String> = repo
        .refs
        .branches(RELEASE_BRANCH)?
        .into_iter()
        .filter(|branch| branch.origin.is_none() && releases::line_of(&branch.name).is_some())
        .map(|branch| branch.name)
        .collect();
    let tags = repo.refs.tags()?;
    let tags: Vec<String> = tags
        .local
        .keys()
        .filter(|name| !tags.origin.contains_key(*name))
        .filter(|name| !releases::versions([*name]).is_empty())
        .cloned()
        .collect();
    if branches.is_empty() && tags.is_empty() {
        return Ok(());
    }

    let mut delete = vec![];
    if !branches.is_empty() {
        delete.push(format!("`git branch -D {}`", branches.join(" ")));
    }
    if !tags.is_empty() {
        delete.push(format!("`git tag -d {}`", tags.join(" ")));
    }
    let names: Vec<String> = branches.into_iter().chain(tags).collect();
    let them = if names.len() == 1 { "it" } else { "them" };

    Err(format!(
        "Only this repository has {}, so a cut would take {them} for releases already made. \
         Publish {them} with `git push {ORIGIN} {}`, or delete {them} with {}, and cut again.",
        listed(&names),
        names.join(" "),
        delete.join(" and "),
    ))
}

fn unreleased(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    notes: &Worktree<'_>,
) -> Result<Vec<TopLevel>, String> {
    if !pipeline.fragments_in(notes).is_dir() {
        return Err(format!(
            "`{MAIN}` has no fragments directory at {}, so there is nothing to release.",
            pipeline.fragments
        ));
    }

    let mut files = top_level(pipeline, releases, notes)?;
    files.retain(|file| file.shipping.is_empty());

    Ok(files)
}

/// Load all top-level fragments and highlights in `tree` and resolve which
/// release branches already ship their changes.
pub(crate) fn top_level(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    tree: &Worktree<'_>,
) -> Result<Vec<TopLevel>, String> {
    let dir = pipeline.fragments_in(tree);
    let Contents {
        fragments,
        highlights,
    } = directory::read(&dir, &Part::Unreleased).map_err(|error| match error {
        ReadError::Problems(found) => format!(
            "The fragments on `{MAIN}` break their format; fix these first:\n{}",
            problems(tree, &found)
        ),
        error => error.to_string(),
    })?;
    let history = History::read(&dir).map_err(|error| error.to_string())?;

    let files: Vec<(TopLevel, Option<&[ListedCredit]>)> = fragments
        .iter()
        .map(|fragment| {
            let file = TopLevel {
                path: fragment.path.clone(),
                highlight: false,
                breaking: fragment.file.upgrade.is_some(),
                changes: vec![],
                shipping: vec![],
            };
            (file, fragment.file.credits.as_deref())
        })
        .chain(highlights.iter().map(|highlight| {
            let file = TopLevel {
                path: highlight.path.clone(),
                highlight: true,
                breaking: false,
                changes: vec![],
                shipping: vec![],
            };
            (file, highlight.file.credits.as_deref())
        }))
        .collect();
    let paths: Vec<&str> = files.iter().map(|(file, _)| file.path.as_str()).collect();
    let origins = history.origins(&paths);

    let mut changes = Changes::new(pipeline, releases, tree)?;
    let mut top_level = vec![];
    for ((mut file, credits), origin) in files.into_iter().zip(origins) {
        file.changes = changes.described(&file.path, credits, origin)?;
        file.shipping = changes.shipping(&file.changes)?;
        top_level.push(file);
    }

    Ok(top_level)
}

/// Cached commit and release-branch lookup state for a single worktree.
struct Changes<'p> {
    pipeline: &'p Pipeline<'p>,
    releases: &'p Releases,
    forge: Forge,
    tree: String,
    merged: Option<HashMap<NonZeroU64, String>>,
    shipped_by: HashMap<String, Vec<String>>,
}

impl<'p> Changes<'p> {
    fn new(
        pipeline: &'p Pipeline<'p>,
        releases: &'p Releases,
        tree: &Worktree<'_>,
    ) -> Result<Self, String> {
        Ok(Self {
            pipeline,
            releases,
            forge: pipeline.forge(),
            tree: tree.head()?,
            merged: None,
            shipped_by: HashMap::new(),
        })
    }

    /// Resolve the commit hashes described by `path` from its explicit
    /// `credits` (falling back to `origin`, the commit that added the file).
    fn described(
        &mut self,
        path: &str,
        credits: Option<&[ListedCredit]>,
        origin: Option<String>,
    ) -> Result<Vec<String>, String> {
        let mut changes: Vec<String> = vec![];
        for credit in credits.unwrap_or_default() {
            if let Some(change) = self.change(path, credit)?
                && !changes.contains(&change)
            {
                changes.push(change);
            }
        }
        if changes.is_empty() {
            changes.extend(origin);
        }

        Ok(changes)
    }

    fn change(&mut self, path: &str, credit: &ListedCredit) -> Result<Option<String>, String> {
        let pipeline = self.pipeline;
        let number = match credit {
            ListedCredit::Commit(hash) => {
                return pipeline.repo.history.commit(hash).map(Some).map_err(|_| {
                    format!(
                        "{}/{path} credits {hash}, which names no commit this repository has; \
                         correct its `Credits`.",
                        pipeline.fragments
                    )
                });
            }
            ListedCredit::ChangeRequest(_) if self.forge == Forge::None => return Ok(None),
            ListedCredit::ChangeRequest(number) => number,
        };

        if self.merged.is_none() {
            let log = pipeline.repo.history.first_parent_log(&self.tree)?;
            self.merged = Some(merges(&log, self.forge));
        }
        let merged = self.merged.as_ref().and_then(|merged| merged.get(number));

        merged.cloned().map(Some).ok_or_else(|| {
            format!(
                "{}/{path} credits {}, which no commit in {MAIN}'s history merged; correct its \
                 `Credits`.",
                pipeline.fragments,
                credit.written(self.forge)
            )
        })
    }

    fn shipping(&mut self, changes: &[String]) -> Result<Vec<String>, String> {
        let mut shipping: Option<Vec<String>> = None;
        for change in changes {
            let branches = match self.shipped_by.entry(change.clone()) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(unknown) => {
                    let containing = self.pipeline.repo.history.containing(unknown.key())?;
                    let branches = self
                        .releases
                        .shipping(unknown.key(), &containing)
                        .into_iter()
                        .map(|line| line.branch.clone())
                        .collect();
                    unknown.insert(branches)
                }
            };
            shipping = Some(match shipping {
                None => branches.clone(),
                Some(so_far) => so_far
                    .into_iter()
                    .filter(|branch| branches.contains(branch))
                    .collect(),
            });
        }

        Ok(shipping.unwrap_or_default())
    }
}

/// Map each change request number to the oldest commit in `log` that references
/// it on `forge`.
fn merges(log: &str, forge: Forge) -> HashMap<NonZeroU64, String> {
    let mut merges = HashMap::new();
    for record in log.split('\0') {
        let record = record.trim_start_matches('\n');
        let Some((hash, message)) = record.split_once('\n') else {
            continue;
        };
        if let Some(number) = forge.change_request(message) {
            merges.insert(number, hash.to_owned());
        }
    }

    merges
}

/// Return the number of unreleased top-level fragments (excluding highlights)
/// at commit `main`.
pub(crate) fn unreleased_count(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    main: &str,
) -> Result<usize, String> {
    let notes = pipeline.repo.worktrees.add("due", main)?;
    if !pipeline.fragments_in(&notes).is_dir() {
        return Ok(0);
    }

    let unreleased = unreleased(pipeline, releases, &notes)?;

    Ok(unreleased.iter().filter(|file| !file.highlight).count())
}

fn file_notes(
    pipeline: &Pipeline<'_>,
    notes: &Worktree<'_>,
    unreleased: &[TopLevel],
    version: &Version,
    message: &str,
) -> Result<String, String> {
    for file in unreleased {
        let place = Place::parse(&file.path)
            .ok_or_else(|| format!("{} is no fragment or highlight.", file.path))?;
        let to = Place {
            part: Part::Release(version.clone()),
            ..place
        };

        let from = format!("{}/{}", pipeline.fragments, file.path);
        let to = format!("{}/{}", pipeline.fragments, to.path());
        if let Some(parent) = notes.workspace().join(&to).parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {parent}: {error}"))?;
        }
        notes.git(&["mv", &from, &to])?;
    }

    if let Some(bump) = &pipeline.options.bump {
        notes.bump(bump, version)?;
    }

    notes.commit(message)
}

fn start_branch(pipeline: &Pipeline<'_>, at: &str, version: &Version) -> Result<String, String> {
    let code = pipeline.repo.worktrees.add("cut", at)?;
    // History from before the project kept fragments has no directory to remove.
    code.git(&[
        "rm",
        "-r",
        "--quiet",
        "--ignore-unmatch",
        "--",
        &pipeline.fragments,
    ])?;
    if let Some(bump) = &pipeline.options.bump {
        code.bump(bump, version)?;
    }

    code.commit(&releases::start_message(version))
}

#[cfg(test)]
#[path = "cut_tests.rs"]
mod tests;
