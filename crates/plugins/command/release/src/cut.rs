//! `jp release cut`: start the next minor or major release from `main`.
//!
//! `main` is the commit the workspace's checkout is at, which has to hold every
//! commit of `origin`'s `main`.
//! The release branch, `release/<major>.<minor>`, starts there, or with `--at`
//! at an earlier commit of the checkout's first-parent line, with one commit
//! that removes the fragments directory, when the tree has one, and runs
//! `bump`.
//! The notes branch, `release-notes/v<version>`, starts at the checkout with
//! one commit that moves the unreleased fragments and highlights whose changes
//! the release branch ships into the release's directory and runs `bump`;
//! merging it into `main` files the release's notes.
//! The code a release ships and the notes it lists are chosen together, however
//! long the notes take to merge.
//! Both branches are made in this repository, and the cut prints the push that
//! publishes them.
//!
//! A fragment or highlight describes the changes its `Credits` name, or else
//! the commit that added it, and a release branch ships a change when it holds
//! the commit, from `main`'s history at its cut or through a pick.
//! One at the top level is unreleased unless a release branch already ships
//! every change it describes; such a file waits for that release's notes
//! branch.
//! A cut refuses while a release branch or `v` tag exists only in this
//! repository, such as one a rehearsal left behind: the cut would count it as a
//! release made.

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
    /// Its path below the fragments directory.
    pub path: String,

    pub highlight: bool,

    /// Whether it has an upgrade section.
    pub breaking: bool,

    /// The changes it describes, as the hashes of the commits that made them:
    /// the ones its `Credits` name, or else the commit that added it; none when
    /// no commit added it at the top level.
    pub changes: Vec<String>,

    /// The release branches that ship every change it describes, by name; none
    /// when it is unreleased.
    pub shipping: Vec<String>,
}

/// Start the next release from `main`, or `requested`, at the commit checked
/// out or at the earlier commit `at`, and say what was made and how to publish
/// it.
///
/// The error says why nothing was started.
pub(crate) fn cut(
    pipeline: &Pipeline<'_>,
    requested: Option<&Version>,
    at: Option<&str>,
) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.refuse_shallow()?;
    repo.fetch()?;
    let checkout = repo.checkout()?;
    refuse_unpublished(repo)?;
    let releases = repo.releases()?;
    let at = cut_point(repo, &checkout, at)?;
    let from = if at == checkout.head {
        checkout.describe()
    } else {
        format!(
            "{}, an earlier commit of {}",
            short(&at),
            checkout.describe()
        )
    };

    let notes = repo.worktree("cut", &checkout.head)?;
    let unreleased = unreleased(pipeline, &releases, &notes)?;
    if !unreleased.iter().any(|file| !file.highlight) {
        return Err(format!(
            "{} has no unreleased fragments, so there is nothing to release.",
            checkout.describe()
        ));
    }
    let shipped = repo.ancestors(&at)?;
    let filed: Vec<TopLevel> = unreleased
        .into_iter()
        .filter(|file| file.changes.iter().all(|change| shipped.contains(change)))
        .collect();
    let fragments = filed.iter().filter(|file| !file.highlight).count();
    if fragments == 0 {
        return Err(format!(
            "None of the unreleased fragments on {} describes a change {} has, so a release cut \
             there would list nothing; write fragments whose `Credits` name its changes first.",
            checkout.describe(),
            short(&at)
        ));
    }
    let version = next_version(pipeline, &releases, &filed, requested)?;

    let branch = releases::branch(&version);
    let notes_branch = releases::notes_branch(&version);
    let existing = repo.branch(&notes_branch)?;
    if existing.local.is_some() || existing.origin.is_some() {
        return Err(format!(
            "{notes_branch} exists already; delete it if an earlier cut left it behind, here with \
             `git branch -D {notes_branch}` or on {ORIGIN} with `git push {ORIGIN} --delete \
             {notes_branch}`."
        ));
    }

    let files = files(fragments, filed.len() - fragments);
    let destination = format!("{}/{version}/", pipeline.fragments);
    let message = format!(
        "chore(release): Cut release {version}\n\n`jp release cut` started release {version} on \
         `{branch}`, from {from}.\n\nThis moves the {files} it ships into `{destination}`, where \
         its notes are prepared{}.\n",
        if pipeline.options.bump.is_some() {
            format!(", and writes the version {version}")
        } else {
            String::new()
        },
    );

    let noted = file_notes(pipeline, &notes, &filed, &version, &message)?;
    let started = start_branch(pipeline, &at, &version)?;
    repo.set_branch(&branch, &started, None)?;
    if let Err(error) = repo.set_branch(&notes_branch, &noted, None) {
        // Without its notes branch, the release branch would block the next cut.
        drop(repo.git(&["branch", "--delete", "--force", &branch]));
        return Err(error);
    }

    let mut said = format!("Started release {version} on {branch}, cut from {from}.\n");
    if let Some(ahead) = checkout.ahead() {
        let mut holding = vec![];
        if !repo.is_ancestor(&at, &checkout.main)? {
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
        "Filed {files} into {destination} on {notes_branch}.\n\nNothing is published yet. Publish \
         both branches with:\n\n{}\nthen prepare the release's notes on {notes_branch}, and merge \
         it into {MAIN} once they are final.\n",
        push(&[branch, notes_branch.clone()])
    ));

    Ok(said)
}

/// The version a cut filing `filed` starts: `requested`, or the next minor
/// release, or from 1.0 on the next major one when a file has an upgrade
/// section.
///
/// The error refuses a version that can't ship a change that requires users to
/// act when one of `filed` has an upgrade section.
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

/// The commit to start the release at: the commit checked out, or `at`, which
/// has to be on its first-parent line and in no release branch yet.
///
/// The error says why `at` can't start a release.
fn cut_point(
    repo: &Repository<'_>,
    checkout: &Checkout,
    at: Option<&str>,
) -> Result<String, String> {
    let Some(at) = at else {
        return Ok(checkout.head.clone());
    };
    let hash = repo.commit(at)?;
    if hash == checkout.head {
        return Ok(hash);
    }

    if !repo.first_parents(&checkout.head)?.contains(&hash) {
        return Err(format!(
            "{} isn't on the first-parent line of {}, so it isn't a commit of {MAIN}'s own \
             history; a release is cut at one of those.",
            short(&hash),
            checkout.describe()
        ));
    }
    let shipping = repo.containing(&hash)?;
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

/// Refuse while a release branch or a `v` tag exists only in this repository,
/// since a cut would count it as a release made.
fn refuse_unpublished(repo: &Repository<'_>) -> Result<(), String> {
    let branches: Vec<String> = repo
        .branches(RELEASE_BRANCH)?
        .into_iter()
        .filter(|branch| branch.origin.is_none() && releases::line_of(&branch.name).is_some())
        .map(|branch| branch.name)
        .collect();
    let tags = repo.tags()?;
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

/// The fragments and highlights at the top level of the fragments directory in
/// `notes` that no release branch ships.
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

/// The fragments and highlights at the top level of the fragments directory in
/// `tree`, which has to have one, each with the release branches that ship the
/// change that added it.
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

/// What the fragments and highlights of one tree describe, and which release
/// branches ship it, each read once.
struct Changes<'p> {
    pipeline: &'p Pipeline<'p>,
    releases: &'p Releases,
    forge: Forge,

    /// The commit whose fragments directory is read.
    tree: String,

    /// The commit that merged each change request on the first-parent line of
    /// [`Self::tree`], read when a credit first names one.
    merged: Option<HashMap<NonZeroU64, String>>,

    /// The release branches that ship each change, by the hash of its commit.
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

    /// The changes the file at `path`, below the fragments directory,
    /// describes: the commits its `credits` name, or else `origin`, the commit
    /// that added it.
    ///
    /// A change request names the commit that merged it, which only a forge can
    /// find, so without one only the commits `credits` name count.
    /// The error names a credit no commit answers to.
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

    /// The commit `credit`, which the file at `path` lists, names; `None` for a
    /// change request without a forge to find it by.
    fn change(&mut self, path: &str, credit: &ListedCredit) -> Result<Option<String>, String> {
        let pipeline = self.pipeline;
        let number = match credit {
            ListedCredit::Commit(hash) => {
                return pipeline.repo.commit(hash).map(Some).map_err(|_| {
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
            let log = pipeline.repo.first_parent_log(&self.tree)?;
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

    /// The release branches that ship every one of `changes`, by name.
    fn shipping(&mut self, changes: &[String]) -> Result<Vec<String>, String> {
        let mut shipping: Option<Vec<String>> = None;
        for change in changes {
            let branches = match self.shipped_by.entry(change.clone()) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(unknown) => {
                    let containing = self.pipeline.repo.containing(unknown.key())?;
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

/// The commit that merged each change request, as `forge` writes them into the
/// commits it merges, read from the log `Repository::first_parent_log` writes,
/// newest first.
///
/// The oldest commit naming a change request is the one that merged it.
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

/// How many fragments at the top level of the fragments directory in the commit
/// `main` no release branch ships; none when it has no fragments directory.
///
/// Highlights aren't counted: a release needs a fragment to list.
pub(crate) fn unreleased_count(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    main: &str,
) -> Result<usize, String> {
    let notes = pipeline.repo.worktree("due", main)?;
    if !pipeline.fragments_in(&notes).is_dir() {
        return Ok(0);
    }

    let unreleased = unreleased(pipeline, releases, &notes)?;

    Ok(unreleased.iter().filter(|file| !file.highlight).count())
}

/// Move each of `unreleased` into the directory of release `version` in
/// `notes`, run `bump`, and commit it all with `message`.
///
/// Returns the commit's hash.
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

/// Make the commit that starts release `version` on its branch, on top of the
/// commit `at`: the fragments directory removed, and `bump` run.
///
/// Returns the commit's hash.
fn start_branch(pipeline: &Pipeline<'_>, at: &str, version: &Version) -> Result<String, String> {
    let code = pipeline.repo.worktree("cut", at)?;
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
