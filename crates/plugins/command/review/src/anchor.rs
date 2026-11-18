//! What a comment is anchored to: the review as a whole, a file, or a range of
//! a file, in one version of the review.
//!
//! `review.json` holds an anchor as its version, its file's path, and its range
//! in the form `--at` takes after the path:
//!
//! ```json
//! { "version": 2, "path": "docs/a.md", "range": "42:5-43:18" }
//! ```

use serde::{Deserialize, Serialize};

use crate::text::Range;

/// What a comment is about, in the version its lines and columns count in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Stored", into = "Stored")]
pub struct Anchor {
    pub version: u32,
    pub target: Target,
}

/// What a comment is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The review as a whole.
    Review,

    /// A whole file, by its path relative to the workspace root.
    File(String),

    /// A range of a file.
    Range(String, Range),
}

/// An anchor as `review.json` holds it.
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

/// Read what `--at` names, `PATH` or `PATH:RANGE`, as a file the review tracks
/// and an optional range in it.
///
/// `find` returns the tracked path that a path, as typed, names; `tracked`
/// lists every tracked path, for the error when none matches.
/// The whole of `at` is tried as a path first, so a file whose name holds a
/// colon can be named alone.
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
