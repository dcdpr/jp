//! `jp release finalize`: tag a release in preparation.
//!
//! The tag, `v<version>`, is annotated and points at the tip of the release
//! branch: this repository's, or `origin`'s when this repository's is behind.
//! It is made in this repository, and pushing it makes the release.
//! The person running it decides when: on GitHub, CI shows its checks on the
//! branch's tip once the branch is pushed.

use semver::Version;

use crate::{
    pipeline::{Pipeline, push, short},
    releases,
    repository::ORIGIN,
};

/// Tag release `version` at the tip of its release branch, and say how to
/// publish the tag.
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
    repo.tag(&tag, &line.tip, &format!("Release {version}"))?;

    let mut said = format!(
        "Tagged the tip of {}, {}, as {tag}.\n",
        line.branch,
        short(&line.tip)
    );
    said.push_str(&publish(pipeline, &line.branch, &tag, true)?);

    Ok(said)
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
