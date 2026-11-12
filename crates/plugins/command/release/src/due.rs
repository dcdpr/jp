//! `jp release due`: whether the next release should be cut.
//!
//! The next release is due once `cadence` has passed since the newest minor or
//! major release was cut, that release has been finalized, and `main` has an
//! unreleased fragment to release.
//! A release is cut when the commit that starts it on its release branch is
//! made.
//! A release tagged without such a commit, such as one made before the
//! pipeline, counts from the date of its tag.
//! Before any release, the first one is due as soon as `main` has an unreleased
//! fragment.

use chrono::DateTime;
use semver::Version;
use serde::Serialize;

use crate::{cut, options::Cadence, pipeline::Pipeline, releases::Releases};

/// Whether the next release is due, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Due {
    pub due: bool,

    /// Why, as one sentence.
    pub reason: String,
}

/// Whether the next release is due at `now`, in seconds since the Unix epoch,
/// from what `origin`'s tags, release branches, and `main` say.
///
/// The error says why they can't be read.
pub(crate) fn due(pipeline: &Pipeline<'_>, now: i64) -> Result<Due, String> {
    let repo = &pipeline.repo;
    repo.origin.refuse_shallow()?;
    repo.origin.fetch()?;
    let releases = repo.releases()?;

    next(pipeline, &releases, now, &repo.origin.main()?)
}

/// Whether the next release is due at `now`, in seconds since the Unix epoch,
/// after `releases`, with the commit `main` standing for `main`.
///
/// The error says why they can't be read.
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

    // `cut` refuses to start a release without an unreleased fragment, so a
    // reminder then would ask for a release nobody can make.
    if cut::unreleased_count(pipeline, releases, main)? == 0 {
        return Ok(nothing_to_release(releases.newest_cut().is_none()));
    }

    Ok(by_cadence)
}

/// The answer when a release would be due but `main` has no unreleased
/// fragment; `first` when no release has been cut yet.
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

/// Whether the release after `release` is due at `now`: `release` was cut at
/// `cut` and has been finalized, and the next one is due `cadence` later.
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

/// The day of `at`, in seconds since the Unix epoch, in UTC, such as
/// `2026-10-21`.
fn day(at: i64) -> String {
    utc(at, "%Y-%m-%d")
}

/// `at`, in seconds since the Unix epoch, to the minute in UTC, such as
/// `2026-11-04 12:00 UTC`.
fn moment(at: i64) -> String {
    utc(at, "%Y-%m-%d %H:%M UTC")
}

/// `at`, in seconds since the Unix epoch, in UTC as `pattern` writes it; the
/// number itself for a time past what a date can hold.
fn utc(at: i64, pattern: &str) -> String {
    DateTime::from_timestamp(at, 0)
        .map_or_else(|| at.to_string(), |time| time.format(pattern).to_string())
}

#[cfg(test)]
#[path = "due_tests.rs"]
mod tests;
