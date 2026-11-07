//! `jp release finalize`: tag a release in preparation.
//!
//! The tag, `v<version>`, is annotated and points at the tip of the release
//! branch: this repository's, or `origin`'s when this repository's is behind.
//! Its message is the release notes, followed by the upgrade guide when the
//! release has one, as the commit checked out has them in the release's
//! directory; that commit stands for `main`, so the notes branch has to be
//! merged into it first.
//! The tag is made in this repository, and pushing it makes the release.
//! The person running it decides when: on GitHub, CI shows its checks on the
//! branch's tip once the branch is pushed.

use changelog::{
    ReleaseError,
    directory::{Part, ReadError},
    markdown, release,
};
use semver::Version;

use crate::{
    cut,
    pipeline::{Pipeline, counted, listed, problems, push, short},
    releases::{self, Line, Releases},
    repository::{MAIN, ORIGIN, Worktree},
};

/// The heading the upgrade guide follows the release notes under, in a tag's
/// message.
const UPGRADE_GUIDE: &str = "Upgrade guide";

/// Where the notes of a release in preparation stand, in a commit that stands
/// for `main`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notes {
    /// Its notes branch has files of the release's directory the commit lacks,
    /// each relative to the workspace root.
    Unmerged {
        /// The notes branch as a sentence names it: `release-notes/v0.3.0`, or
        /// `origin's release-notes/v0.3.0`.
        branch: String,

        /// The command that deletes that branch.
        delete: String,

        missing: Vec<String>,
    },

    /// Fragments or highlights at the top level of the fragments directory
    /// whose change the release's branch ships, each relative to the workspace
    /// root.
    Unfiled(Vec<String>),

    /// The commit has no notes of the release.
    Missing,

    /// The commit has the release's notes.
    Filed,
}

/// Tag release `version` at the tip of its release branch, with its notes as
/// the commit checked out has them, and say how to publish the tag.
///
/// The error says why `version` can't be finalized.
pub(crate) fn finalize(pipeline: &Pipeline<'_>, version: &Version) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.refuse_shallow()?;
    repo.fetch()?;
    let releases = repo.releases()?;
    let tag = releases::tag(version);

    // A tag made here and not yet pushed is the release, unless it is
    // somewhere else.
    if !releases.published.contains(version)
        && let Some(tagged) = repo.local_tag(&tag)?
    {
        let Some(line) = releases.line(version) else {
            return Err(format!(
                "This repository has a tag {tag}, but no {} to check it against; delete it with \
                 `git tag -d {tag}` if a rehearsal left it.",
                releases::branch(version)
            ));
        };
        if tagged != line.tip {
            return Err(format!(
                "This repository has a tag {tag} at {}, not at the tip of {}, {}; delete it with \
                 `git tag -d {tag}`.",
                short(&tagged),
                line.branch,
                short(&line.tip)
            ));
        }

        return publish(pipeline, &line.branch, &tag, false);
    }

    let line = releases.finalize_target(version)?;
    let checkout = repo.checkout()?;
    let main = repo.worktree("finalize", &checkout.head)?;
    let at = checkout.describe();
    let merge = format!(
        "Merge {} into {MAIN} first, then finalize from there.",
        releases::notes_branch(version)
    );
    match notes(pipeline, &releases, line, version, &main)? {
        Notes::Filed => {}
        Notes::Unmerged {
            branch,
            delete,
            missing,
        } => {
            return Err(format!(
                "{at} lacks {} of the notes of {version} that {branch} has: {}. {merge} If {MAIN} \
                 dropped {} on purpose, delete the branch with {delete}.",
                counted(missing.len(), "file", "files"),
                listed(&missing),
                if missing.len() == 1 { "it" } else { "them" },
            ));
        }
        Notes::Unfiled(pending) => {
            return Err(format!(
                "{at} still has {} at the top of {} whose changes {} ships: {}. Their notes \
                 aren't filed yet. {merge}",
                counted(pending.len(), "file", "files"),
                pipeline.fragments,
                line.branch,
                listed(&pending),
            ));
        }
        Notes::Missing => {
            let directory = format!("{}/{version}/", pipeline.fragments);
            return Err(format!(
                "{at} has no notes of {version} in {directory}. Write at least one fragment \
                 there, such as {directory}<slug>.fixed.md, and finalize from a commit that has \
                 it."
            ));
        }
    }
    let message = message(pipeline, version, &main)?;
    repo.tag(&tag, &line.tip, &message)?;

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

/// Where the notes of `version`, in preparation on `line`, stand in the commit
/// `main` is a worktree of.
///
/// They are unmerged while its notes branch, here or on `origin`, has files of
/// the release's directory the commit lacks, and unfiled while fragments or
/// highlights whose change `line` ships sit at the top level of the fragments
/// directory.
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
    let filed = repo.files_at(&main.head()?, &directory)?;

    let branch = repo.branch(&notes_branch)?;
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

/// The message of the tag of `version`: the subject `Release <version>`, then
/// its release notes as `main` has them, then its upgrade guide under a heading
/// of its own when it has one.
fn message(
    pipeline: &Pipeline<'_>,
    version: &Version,
    main: &Worktree<'_>,
) -> Result<String, String> {
    let options = pipeline.options;
    let release = release::read(
        &pipeline.fragments_in(main),
        &Part::Release(version.clone()),
        options.forge,
        &options.headings,
    )
    .map_err(|error| match error {
        ReleaseError::Read(ReadError::Problems(found)) => format!(
            "The notes of {version} break their format; fix these first:\n{}",
            problems(main, &found)
        ),
        error => error.to_string(),
    })?;

    // The upgrade guide follows in the same message, so a breaking entry links
    // to its section by anchor alone.
    let notes = markdown::notes(&release, &options.headings, "");
    let guide = markdown::upgrade_guide(&release);

    let body = if guide.is_empty() {
        notes
    } else {
        format!("{notes}\n# {UPGRADE_GUIDE}\n\n{guide}")
    };

    // The first paragraph is the tag's subject, which `git tag -n` lists.
    Ok(format!("Release {version}\n\n{body}"))
}

/// How to publish the tag `tag`, at the tip of `branch`, which this command
/// made when `made`; with `branch` too when `origin` lacks commits of it.
fn publish(pipeline: &Pipeline<'_>, branch: &str, tag: &str, made: bool) -> Result<String, String> {
    let repo = &pipeline.repo;
    let standing = repo.standing(&repo.branch(branch)?)?;
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
