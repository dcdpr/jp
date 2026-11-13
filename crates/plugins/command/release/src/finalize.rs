//! `jp release finalize`: creating the annotated `v<version>` tag for a release
//! in preparation.
//!
//! Creates an annotated tag `v<version>` at the tip of the release branch
//! (`release/<major>.<minor>`), using the rendered release notes and upgrade
//! guide from `<fragments_dir>/<version>/` in the current checkout as the tag
//! message.
//! Requires `release-notes/v<version>` to be merged into the current checkout
//! first.
//!
//! Does not push the tag; prints the `git push` command to publish it.

use changelog::{
    NoAddress, ReleaseError,
    directory::{Part, ReadError},
    markdown, release,
};
use release_options::no_address;
use semver::Version;

use crate::{
    cut,
    pipeline::{Pipeline, counted, listed, problems, push, short},
    releases::{self, Line, Releases},
    repository::{MAIN, ORIGIN, Worktree},
};

const UPGRADE_GUIDE: &str = "Upgrade guide";

/// Status of a release's notes in a checkout representing `main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notes {
    /// `release-notes/v<version>` contains files in
    /// `<fragments_dir>/<version>/` that are not yet present in the checkout.
    Unmerged {
        branch: String,
        delete: String,
        missing: Vec<String>,
    },

    /// Top-level fragments or highlights whose changes are shipped by the
    /// release branch have not yet been moved into
    /// `<fragments_dir>/<version>/`.
    Unfiled(Vec<String>),

    /// `<fragments_dir>/<version>/` has no fragments or highlights in the
    /// checkout.
    Missing,

    /// All notes for the release are filed in `<fragments_dir>/<version>/`.
    Filed,
}

/// Create the annotated tag `v<version>` at the tip of its release branch and
/// return a summary with the `git push` command to publish it.
pub(crate) fn finalize(pipeline: &Pipeline<'_>, version: &Version) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.origin.refuse_shallow()?;
    repo.origin.fetch()?;
    let releases = repo.releases()?;
    let tag = releases::tag(version);
    if let Some(said) = tagged_here(pipeline, &releases, version, &tag)? {
        return Ok(said);
    }

    let line = releases.finalize_target(version)?;
    let checkout = repo.origin.checkout()?;
    let main = repo.worktrees.add("finalize", &checkout.head)?;
    let notes = notes(pipeline, &releases, line, version, &main)?;
    refuse_unready(pipeline, notes, &checkout.describe(), version, line)?;
    let message = message(pipeline, version, &main)?;
    repo.refs.tag(&tag, &line.tip, &message)?;

    let mut said = format!(
        "Tagged the tip of {}, {}, as {tag}, with the notes of {version} on {}.\n",
        line.branch,
        short(&line.tip),
        checkout.describe()
    );
    if let Some(ahead) = checkout.ahead() {
        said.push_str(&format!(
            "{ahead}, so the tag's notes may not match {ORIGIN}'s {MAIN}.\n"
        ));
    }
    said.push_str(&publish(pipeline, &line.branch, &tag, true)?);

    Ok(said)
}

/// If `tag` already exists locally (and not yet on `origin`) at the tip of its
/// release branch, return the push instructions for it.
fn tagged_here(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    version: &Version,
    tag: &str,
) -> Result<Option<String>, String> {
    if releases.published.contains(version) {
        return Ok(None);
    }
    let Some(tagged) = pipeline.repo.refs.local_tag(tag)? else {
        return Ok(None);
    };

    let Some(line) = releases.line(version) else {
        return Err(format!(
            "This repository has a tag {tag}, but no {} to check it against; delete it with `git \
             tag -d {tag}` if a rehearsal left it.",
            releases::branch(version)
        ));
    };
    if tagged != line.tip {
        return Err(format!(
            "This repository has a tag {tag} at {}, not at the tip of {}, {}; delete it with `git \
             tag -d {tag}`.",
            short(&tagged),
            line.branch,
            short(&line.tip)
        ));
    }

    publish(pipeline, &line.branch, tag, false).map(Some)
}

fn refuse_unready(
    pipeline: &Pipeline<'_>,
    notes: Notes,
    at: &str,
    version: &Version,
    line: &Line,
) -> Result<(), String> {
    let merge = format!(
        "Merge {} into {MAIN} first, then finalize from there.",
        releases::notes_branch(version)
    );

    match notes {
        Notes::Filed => Ok(()),
        Notes::Unmerged {
            branch,
            delete,
            missing,
        } => Err(format!(
            "{at} lacks {} of the notes of {version} that {branch} has: {}. {merge} If {MAIN} \
             dropped {} on purpose, delete the branch with {delete}.",
            counted(missing.len(), "file", "files"),
            listed(&missing),
            if missing.len() == 1 { "it" } else { "them" },
        )),
        Notes::Unfiled(pending) => Err(format!(
            "{at} still has {} at the top of {} whose changes {} ships: {}. Their notes aren't \
             filed yet. {merge}",
            counted(pending.len(), "file", "files"),
            pipeline.fragments,
            line.branch,
            listed(&pending),
        )),
        Notes::Missing => {
            let directory = format!("{}/{version}/", pipeline.fragments);
            Err(format!(
                "{at} has no notes of {version} in {directory}. Write at least one fragment \
                 there, such as {directory}<slug>.fixed.md, and finalize from a commit that has \
                 it."
            ))
        }
    }
}

/// Check the status of `version`'s release notes in `main`.
pub(crate) fn notes(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    line: &Line,
    version: &Version,
    main: &Worktree<'_>,
) -> Result<Notes, String> {
    let repo = &pipeline.repo;
    let notes_branch = releases::notes_branch(version);
    let directory = format!("{}/{version}/", pipeline.fragments);
    let filed = repo.history.files_at(&main.head()?, &directory)?;

    let branch = repo.refs.branch(&notes_branch)?;
    for (hash, name, delete) in [
        (
            branch.local,
            notes_branch.clone(),
            format!("`git branch -D {notes_branch}`"),
        ),
        (
            branch.origin,
            format!("{ORIGIN}'s {notes_branch}"),
            format!("`git push {ORIGIN} --delete {notes_branch}`"),
        ),
    ] {
        let Some(hash) = hash else {
            continue;
        };
        let missing: Vec<String> = repo
            .history
            .files_at(&hash, &directory)?
            .difference(&filed)
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Ok(Notes::Unmerged {
                branch: name,
                delete,
                missing,
            });
        }
    }

    if pipeline.fragments_in(main).is_dir() {
        let pending: Vec<String> = cut::top_level(pipeline, releases, main)?
            .into_iter()
            .filter(|file| file.shipping.contains(&line.branch))
            .map(|file| format!("{}/{}", pipeline.fragments, file.path))
            .collect();
        if !pending.is_empty() {
            return Ok(Notes::Unfiled(pending));
        }
    }

    Ok(if filed.is_empty() {
        Notes::Missing
    } else {
        Notes::Filed
    })
}

/// Render the annotated tag message for `version` (`Release <version>` subject
/// followed by the release notes and optional upgrade guide).
fn message(
    pipeline: &Pipeline<'_>,
    version: &Version,
    main: &Worktree<'_>,
) -> Result<String, String> {
    let options = pipeline.options;
    let release = release::read(
        &pipeline.fragments_in(main),
        &Part::Release(version.clone()),
        options.notes.forge,
        options.notes.repository.as_deref(),
        &options.notes.headings,
    )
    .map_err(|error| match error {
        ReleaseError::Read(ReadError::Problems(found)) => format!(
            "The notes of {version} break their format; fix these first:\n{}",
            problems(main, &found)
        ),
        ReleaseError::NoAddress(NoAddress(forge)) => no_address(forge),
        error => error.to_string(),
    })?;

    // Pass an empty upgrade_guide base path so breaking entries link to
    // `#<anchor>` within the same tag message.
    let notes = markdown::notes(&release, &options.notes.headings, "");
    let guide = markdown::upgrade_guide(&release);

    let body = if guide.is_empty() {
        notes
    } else {
        format!("{notes}\n# {UPGRADE_GUIDE}\n\n{guide}")
    };

    Ok(format!("Release {version}\n\n{body}"))
}

fn publish(pipeline: &Pipeline<'_>, branch: &str, tag: &str, made: bool) -> Result<String, String> {
    let repo = &pipeline.repo;
    let standing = repo.refs.standing(&repo.refs.branch(branch)?)?;
    let unpushed = standing.is_some_and(releases::Standing::unpushed);

    let mut refs = vec![];
    if unpushed {
        refs.push(branch.to_owned());
    }
    refs.push(tag.to_owned());

    let mut said = String::new();
    if !made {
        said.push_str(&format!(
            "{tag} is tagged at the tip of {branch} already, but {ORIGIN} doesn't have it.\n"
        ));
    }
    if unpushed {
        said.push_str(&format!(
            "{ORIGIN} lacks commits of {branch}, so its CI hasn't checked what {tag} points at.\n"
        ));
    }
    said.push_str(&format!(
        "\nNothing is published yet. Push the tag to release it:\n\n{}",
        push(&refs)
    ));

    Ok(said)
}

#[cfg(test)]
#[path = "finalize_tests.rs"]
mod tests;
