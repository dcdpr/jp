//! `jp release cut`: start the next minor or major release from `main`.
//!
//! The release branch, `release/<major>.<minor>`, starts at the tip of
//! `origin`'s `main` with one commit that removes the fragments directory and
//! runs `bump`.
//! A branch from the same commit, `release-cut/v<version>`, moves every
//! unreleased fragment and highlight into the release's directory and runs
//! `bump`, and becomes the change request that files the release's notes on
//! `main`.
//! Both start at one commit, so the code the release ships and the fragments it
//! lists are chosen together, however long the change request takes to merge.
//!
//! A fragment at the top level is unreleased unless a release branch already
//! ships the change that added it, from `main`'s history at its cut or through
//! a pick; such a fragment waits for that release's change request.

use std::{collections::HashMap, fs};

use changelog::{
    History, Place,
    directory::{self, Contents, Part, ReadError},
};
use semver::Version;

use crate::{
    change_request::ChangeRequest,
    pipeline::{Pipeline, files, problems, short},
    releases::{self, Releases},
    repository::{MAIN, Worktree},
};

/// A fragment or highlight a cut files into the release's directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Unreleased {
    /// Its path below the fragments directory.
    path: String,

    highlight: bool,

    /// Whether it has an upgrade section.
    breaking: bool,
}

/// Start the next release from `main`, or `requested`, and say what was done.
///
/// The error says why nothing was started, or what was done before a step
/// failed.
pub(crate) fn cut(pipeline: &Pipeline<'_>, requested: Option<&Version>) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.refuse_shallow()?;
    repo.fetch()?;
    let releases = repo.releases()?;
    let main = repo.main()?;

    let notes = repo.worktree("cut", &main)?;
    let unreleased = unreleased(pipeline, &releases, &notes)?;
    let fragments = unreleased.iter().filter(|file| !file.highlight).count();
    if fragments == 0 {
        return Err(format!(
            "`{MAIN}` has no unreleased fragments, so there is nothing to release."
        ));
    }
    let breaking = unreleased.iter().any(|file| file.breaking);
    let version = releases.next_cut(breaking, requested)?;

    let branch = releases::branch(&version);
    let cut_branch = format!("release-cut/v{version}");
    if repo.has_branch(&cut_branch)? {
        return Err(format!(
            "{cut_branch} exists on origin already; delete it if an earlier cut left it behind."
        ));
    }
    let opener = pipeline.opener()?;

    let files = files(fragments, unreleased.len() - fragments);
    let destination = format!("{}/{version}/", pipeline.fragments);
    let request = ChangeRequest {
        branch: cut_branch.clone(),
        title: format!("chore(release): Cut release {version}"),
        body: format!(
            "`jp release cut` started release {version} on `{branch}`, from `{MAIN}` at \
             {}.\n\nThis moves the {files} it ships into `{destination}`, where its notes are \
             prepared{}.\n",
            short(&main),
            if pipeline.options.bump.is_some() {
                format!(", and writes the version {version}")
            } else {
                String::new()
            },
        ),
    };

    let filed = file_notes(pipeline, &notes, &unreleased, &version, &request)?;
    let started = start_branch(pipeline, &main, &version)?;

    repo.push(&started, &branch, true, &[])?;
    let pushed = repo
        .push(&filed, &cut_branch, true, &opener.push_options(&request))
        .map_err(|error| {
            format!("Started release {version} on {branch}, but its notes weren't filed: {error}")
        })?;
    let opened = opener.open(&request, &pushed).map_err(|error| {
        format!(
            "Started release {version} on {branch} and pushed {cut_branch}, but its change \
             request wasn't opened: {error}"
        )
    })?;

    Ok(format!(
        "Started release {version} on {branch}, cut from {MAIN} at {}.\nPushed {cut_branch}, \
         which files {files} into {destination}.\n{opened}.\n",
        short(&main)
    ))
}

/// The fragments and highlights at the top level of the fragments directory in
/// `notes` that no release branch ships.
fn unreleased(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    notes: &Worktree<'_>,
) -> Result<Vec<Unreleased>, String> {
    let dir = pipeline.fragments_in(notes);
    if !dir.is_dir() {
        return Err(format!(
            "`{MAIN}` has no fragments directory at {}, so there is nothing to release.",
            pipeline.fragments
        ));
    }

    let Contents {
        fragments,
        highlights,
    } = directory::read(&dir, &Part::Unreleased).map_err(|error| match error {
        ReadError::Problems(found) => format!(
            "The fragments on `{MAIN}` break their format; fix these first:\n{}",
            problems(notes, &found)
        ),
        error => error.to_string(),
    })?;
    let history = History::read(&dir).map_err(|error| error.to_string())?;

    let files: Vec<Unreleased> = fragments
        .iter()
        .map(|fragment| Unreleased {
            path: fragment.path.clone(),
            highlight: false,
            breaking: fragment.file.upgrade.is_some(),
        })
        .chain(highlights.iter().map(|highlight| Unreleased {
            path: highlight.path.clone(),
            highlight: true,
            breaking: false,
        }))
        .collect();
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    let origins = history.origins(&paths);

    let mut shipped: HashMap<String, bool> = HashMap::new();
    let mut unreleased = vec![];
    for (file, origin) in files.into_iter().zip(origins) {
        if let Some(hash) = origin {
            let ships = if let Some(&ships) = shipped.get(&hash) {
                ships
            } else {
                let containing = pipeline.repo.containing(&hash)?;
                let ships = !releases.shipping(&hash, &containing).is_empty();
                shipped.insert(hash, ships);
                ships
            };
            if ships {
                continue;
            }
        }
        unreleased.push(file);
    }

    Ok(unreleased)
}

/// Move each of `unreleased` into the directory of release `version` in
/// `notes`, run `bump`, and commit it all as `request` describes.
///
/// Returns the commit's hash.
fn file_notes(
    pipeline: &Pipeline<'_>,
    notes: &Worktree<'_>,
    unreleased: &[Unreleased],
    version: &Version,
    request: &ChangeRequest,
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

    notes.commit(&format!("{}\n\n{}", request.title, request.body))
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
