//! `jp release finalize`: tag a release in preparation.
//!
//! The tag, `v<version>`, is annotated and points at the tip of `origin`'s
//! release branch, and pushing it makes the release.
//! The person running it decides when: on GitHub, CI shows its checks on the
//! branch's tip.

use semver::Version;

use crate::pipeline::{Pipeline, short};

/// Tag release `version` at the tip of its release branch and push the tag, and
/// say what was done.
///
/// The error says why `version` can't be finalized.
pub(crate) fn finalize(pipeline: &Pipeline<'_>, version: &Version) -> Result<String, String> {
    let repo = &pipeline.repo;
    repo.refuse_shallow()?;
    repo.fetch()?;
    let releases = repo.releases()?;
    let line = releases.finalize_target(version)?;

    let tag = format!("v{version}");
    match repo.commit(&format!("refs/tags/{tag}")) {
        // A run that tagged here and failed to push left it.
        Ok(tagged) if tagged == line.tip => {}
        Ok(tagged) => {
            return Err(format!(
                "This repository has a tag {tag} at {}, not at the tip of {}; delete it with `git \
                 tag --delete {tag}`.",
                short(&tagged),
                line.branch
            ));
        }
        Err(_) => {
            let message = format!("Release {version}");
            repo.git(&["tag", "--annotate", "--message", &message, &tag, &line.tip])?;
        }
    }
    repo.push_tag(&tag)?;

    Ok(format!(
        "Tagged the tip of {}, {}, as {tag}, and pushed it.\n",
        line.branch,
        short(&line.tip)
    ))
}

#[cfg(test)]
#[path = "finalize_tests.rs"]
mod tests;
