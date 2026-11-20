//! Comment anchors: the review as a whole, a file, or a line/column range in a
//! specific version of a file.
//!
//! In `review.json`, an anchor is serialized as `{ version, path?, range? }`,
//! where `range` uses the same syntax as `--at` after the path:
//!
//! ```json
//! { "version": 2, "path": "docs/a.md", "range": "42:5-43:18" }
//! ```

use serde::{Deserialize, Serialize};

use crate::text::Range;

/// Target of a comment and the version in which its line and column numbers are
/// measured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Stored", into = "Stored")]
pub struct Anchor {
    pub version: u32,
    pub target: Target,
}

/// Target of a comment within a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The review as a whole.
    Review,

    /// An entire file, named relative to the workspace root.
    File(String),

    /// A line or character range within a file.
    Range(String, Range),
}

/// Serialized representation of [`Anchor`] in `review.json`.
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    range: Option<Range>,
}

impl TryFrom<Stored> for Anchor {
    type Error = String;

    fn try_from(stored: Stored) -> Result<Self, Self::Error> {
        let target = match (stored.path, stored.range) {
            (None, None) => Target::Review,
            (Some(path), None) => Target::File(path),
            (Some(path), Some(range)) => Target::Range(path, range),
            (None, Some(range)) => return Err(format!("the anchor's range {range} names no file")),
        };

        Ok(Self {
            version: stored.version,
            target,
        })
    }
}

impl From<Anchor> for Stored {
    fn from(anchor: Anchor) -> Self {
        let (path, range) = match anchor.target {
            Target::Review => (None, None),
            Target::File(path) => (Some(path), None),
            Target::Range(path, range) => (Some(path), Some(range)),
        };

        Self {
            version: anchor.version,
            path,
            range,
        }
    }
}

/// Parse an `--at` argument (`PATH` or `PATH:RANGE`) into a tracked file path
/// and an optional [`Range`].
///
/// `find` resolves a user-supplied path to a tracked path; `tracked` lists all
/// tracked paths for the error message when no path matches.
/// Tries matching all of `at` as a file path first so filenames containing `:`
/// can be referenced without a range.
pub fn place(
    at: &str,
    tracked: &[String],
    find: impl Fn(&str) -> Option<String>,
) -> Result<(String, Option<Range>), String> {
    if let Some(path) = find(at) {
        return Ok((path, None));
    }

    let mut refused = None;
    for (index, _) in at.match_indices(':') {
        let Some(path) = find(&at[..index]) else {
            continue;
        };
        match at[index + 1..].parse() {
            Ok(range) => return Ok((path, Some(range))),
            Err(error) => refused = Some(error),
        }
    }

    Err(refused.unwrap_or_else(|| {
        format!(
            "`{at}` names no file in the review, which tracks {}.",
            tracked.join(", ")
        )
    }))
}

#[cfg(test)]
#[path = "anchor_tests.rs"]
mod tests;
