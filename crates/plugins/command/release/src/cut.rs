//! `jp release cut`: start the next minor or major release from `main`.
//!
//! `main` is the commit the workspace's checkout is at, which has to hold every
//! commit of `origin`'s `main`.
//! The release branch, `release/<major>.<minor>`, starts there with one commit
//! that removes the fragments directory and runs `bump`.
//! The notes branch, `release-notes/v<version>`, starts at the same commit with
//! one that moves every unreleased fragment and highlight into the release's
//! directory and runs `bump`; merging it into `main` files the release's notes.
//! Both start at one commit, so the code the release ships and the fragments it
//! lists are chosen together, however long the notes take to merge.
//! Both are made in this repository, and the cut prints the push that publishes
//! them.
//!
//! A fragment at the top level is unreleased unless a release branch already
//! ships the change that added it, from `main`'s history at its cut or through
//! a pick; such a fragment waits for that release's notes branch.
//! A cut refuses while a release branch or `v` tag exists only in this
//! repository, such as one a rehearsal left behind: the cut would count it as a
//! release made.

use std::{
    collections::{HashMap, hash_map::Entry},
    fs,
};

use changelog::{
    History, Place,
    directory::{self, Contents, Part, ReadError},
};
use semver::Version;

use crate::{
    pipeline::{Pipeline, files, listed, problems, push},
    releases::{self, RELEASE_BRANCH, Releases},
    repository::{MAIN, ORIGIN, Repository, Worktree},
};

/// A fragment or highlight at the top level of the fragments directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopLevel {
    /// Its path below the fragments directory.
    pub path: String,

    pub highlight: bool,

    /// Whether it has an upgrade section.
    pub breaking: bool,

    /// The release branches that ship the change that added it, by name; none
    /// when it is unreleased.
    pub shipping: Vec<String>,
}

/// Start the next release from `main`, or `requested`, and say what was made
/// and how to publish it.
///
/// The error says why nothing was started.
pub(crate) fn cut(pipeline: &Pipeline<'_>, requested: Option<&Version>) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.refuse_shallow()?;
    repo.fetch()?;
    let checkout = repo.checkout()?;
    refuse_unpublished(repo)?;
    let releases = repo.releases()?;

    let notes = repo.worktree("cut", &checkout.head)?;
    let unreleased = unreleased(pipeline, &releases, &notes)?;
    let fragments = unreleased.iter().filter(|file| !file.highlight).count();
    if fragments == 0 {
        return Err(format!(
            "{} has no unreleased fragments, so there is nothing to release.",
            checkout.describe()
        ));
    }
    let breaking: Vec<&str> = unreleased
        .iter()
        .filter(|file| file.breaking)
        .map(|file| file.path.as_str())
        .collect();
    let version = releases.next_cut(!breaking.is_empty(), requested)?;
    if let Some(first) = breaking.first()
        && !releases::can_break(&version)
    {
        let which = match breaking.len() {
            1 => format!("{}/{first} has an upgrade section", pipeline.fragments),
            count => format!(
                "{}/{first} and {} more have upgrade sections",
                pipeline.fragments,
                count - 1
            ),
        };
        return Err(format!(
            "{version} is a minor release from 1.0 on, which can't ship a change that requires \
             users to act, and {which}. Leave out `--version` to cut the next major release."
        ));
    }

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

    let files = files(fragments, unreleased.len() - fragments);
    let destination = format!("{}/{version}/", pipeline.fragments);
    let message = format!(
        "chore(release): Cut release {version}\n\n`jp release cut` started release {version} on \
         `{branch}`, from {}.\n\nThis moves the {files} it ships into `{destination}`, where its \
         notes are prepared{}.\n",
        checkout.describe(),
        if pipeline.options.bump.is_some() {
            format!(", and writes the version {version}")
        } else {
            String::new()
        },
    );

    let filed = file_notes(pipeline, &notes, &unreleased, &version, &message)?;
    let started = start_branch(pipeline, &checkout.head, &version)?;
    repo.set_branch(&branch, &started, None)?;
    if let Err(error) = repo.set_branch(&notes_branch, &filed, None) {
        // Without its notes branch, the release branch would block the next cut.
        drop(repo.git(&["branch", "--delete", "--force", &branch]));
        return Err(error);
    }

    let mut said = format!(
        "Started release {version} on {branch}, cut from {}.\n",
        checkout.describe()
    );
    if let Some(ahead) = checkout.ahead() {
        said.push_str(&format!(
            "{ahead}: {branch} and {notes_branch} hold commits {MAIN} doesn't have yet.\n"
        ));
    }
    said.push_str(&format!(
        "Filed {files} into {destination} on {notes_branch}.\n\nNothing is published yet. Publish \
         both branches with:\n\n{}\nthen open a pull request from {notes_branch} into {MAIN}, \
         where the release's notes are prepared.\n",
        push(&[branch, notes_branch.clone()])
    ));

    Ok(said)
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

    let files: Vec<TopLevel> = fragments
        .iter()
        .map(|fragment| TopLevel {
            path: fragment.path.clone(),
            highlight: false,
            breaking: fragment.file.upgrade.is_some(),
            shipping: vec![],
        })
        .chain(highlights.iter().map(|highlight| TopLevel {
            path: highlight.path.clone(),
            highlight: true,
            breaking: false,
            shipping: vec![],
        }))
        .collect();
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let origins = history.origins(&paths);

    let mut shipping: HashMap<String, Vec<String>> = HashMap::new();
    let mut top_level = vec![];
    for (mut file, origin) in files.into_iter().zip(origins) {
        if let Some(hash) = origin {
            let branches = match shipping.entry(hash) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(unknown) => {
                    let containing = pipeline.repo.containing(unknown.key())?;
                    let branches = releases
                        .shipping(unknown.key(), &containing)
                        .into_iter()
                        .map(|line| line.branch.clone())
                        .collect();
                    unknown.insert(branches)
                }
            };
            file.shipping.clone_from(branches);
        }
        top_level.push(file);
    }

    Ok(top_level)
}

/// How many fragments at the top level of the fragments directory on `origin`'s
/// `main` no release branch ships; none when `main` has no fragments directory.
///
/// Highlights aren't counted: a release needs a fragment to list.
pub(crate) fn unreleased_count(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
) -> Result<usize, String> {
    let repo = &pipeline.repo;
    let notes = repo.worktree("due", &repo.main()?)?;
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

/// Make the commit that starts release `version` on its branch, on top of
/// `main`: the fragments directory removed, and `bump` run.
///
/// Returns the commit's hash.
fn start_branch(pipeline: &Pipeline<'_>, main: &str, version: &Version) -> Result<String, String> {
    let code = pipeline.repo.worktree("cut", main)?;
    code.git(&["rm", "-r", "--quiet", "--", &pipeline.fragments])?;
    if let Some(bump) = &pipeline.options.bump {
        code.bump(bump, version)?;
    }

    code.commit(&releases::start_message(version))
}

#[cfg(test)]
#[path = "cut_tests.rs"]
mod tests;
