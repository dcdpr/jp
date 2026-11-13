//! `jp release due`: checking whether the next minor or major release is due.
//!
//! A release is due when:
//!
//! 1. No minor or major release is currently in preparation.
//! 2. At least `cadence` has elapsed since the newest minor or major release
//!    was cut (measured from the commit timestamp of `Starts-Release:
//!    <version>` on its release branch, or from the tag date for releases
//!    tagged without a release branch).
//! 3. `main` has at least one unreleased fragment.

use chrono::DateTime;
use semver::Version;
use serde::Serialize;

use crate::{cut, options::Cadence, pipeline::Pipeline, releases::Releases};

/// Result of checking whether the next release is due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Due {
    pub due: bool,
    pub reason: String,
}

/// Check whether the next release is due at Unix timestamp `now` after fetching
/// from `origin`.
pub(crate) fn due(pipeline: &Pipeline<'_>, now: i64) -> Result<Due, String> {
    let repo = &pipeline.repo;
    repo.origin.refuse_shallow()?;
    repo.origin.fetch()?;
    let releases = repo.releases()?;

    next(pipeline, &releases, now, &repo.origin.main()?)
}

/// Evaluate whether the next release is due at Unix timestamp `now` given
/// `releases` and commit `main`.
pub(crate) fn next(
    pipeline: &Pipeline<'_>,
    releases: &Releases,
    now: i64,
    main: &str,
) -> Result<Due, String> {
    let repo = &pipeline.repo;
    let by_cadence = match releases.newest_cut() {
        None => Due {
            due: true,
            reason: "No release has been cut yet, so the first one is due.".to_owned(),
        },
        Some(newest) if !releases.tagged.contains(newest) => Due {
            due: false,
            reason: format!(
                "{newest} is in preparation, so the next release is due once it is finalized."
            ),
        },
        Some(newest) => {
            let cut_at = match releases.started_at(newest) {
                Some(at) => at,
                None => repo.refs.tag_date(&format!("v{newest}"))?,
            };
            assess(newest, cut_at, now, &pipeline.options.cadence)
        }
    };
    if !by_cadence.due {
        return Ok(by_cadence);
    }

    if cut::unreleased_count(pipeline, releases, main)? == 0 {
        return Ok(nothing_to_release(releases.newest_cut().is_none()));
    }

    Ok(by_cadence)
}

fn nothing_to_release(first: bool) -> Due {
    let which = if first { "first" } else { "next" };

    Due {
        due: false,
        reason: format!(
            "`main` has no unreleased fragments, so there is nothing to release yet; the {which} \
             release is due once a fragment is merged."
        ),
    }
}

fn assess(release: &Version, cut: i64, now: i64, cadence: &Cadence) -> Due {
    let next = cut.saturating_add(cadence.seconds());
    if now >= next {
        return Due {
            due: true,
            reason: format!(
                "{release} was cut on {} and has been finalized, and the cadence is {cadence}, so \
                 the next release is due.",
                day(cut)
            ),
        };
    }

    Due {
        due: false,
        reason: format!(
            "{release} was cut on {}, and the cadence is {cadence}, so the next release is due at \
             {}.",
            day(cut),
            moment(next)
        ),
    }
}

fn day(at: i64) -> String {
    utc(at, "%Y-%m-%d")
}

fn moment(at: i64) -> String {
    utc(at, "%Y-%m-%d %H:%M UTC")
}

fn utc(at: i64, pattern: &str) -> String {
    DateTime::from_timestamp(at, 0)
        .map_or_else(|| at.to_string(), |time| time.format(pattern).to_string())
}

#[cfg(test)]
#[path = "due_tests.rs"]
mod tests;
