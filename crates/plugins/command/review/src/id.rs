//! Review, comment, and conversation identifiers.
//!
//! - [`ReviewId`] is a `doc::id::Id` with prefix `R` (e.g. `R-1h4k2m9`); its
//!   7-character body names the review's directory in the store.
//! - [`CommentRef`] identifies a comment by review ID and 1-based number
//!   (`R-1h4k2m9#2`).
//! - [`Conversation`] parses a JP conversation ID in any accepted input form
//!   and formats it as `jp-c<timestamp>`.

use std::{fmt, str::FromStr};

use doc::id::{Id, Kind};
use jp_id::{
    Parts,
    parts::{TargetId, Variant},
};
use serde::{Serialize, Serializer};

/// The kind of document a [`ReviewId`] names.
#[derive(Debug)]
pub enum ReviewKind {}

impl Kind for ReviewKind {
    const PREFIX: char = 'R';
    const NOUN: &'static str = "review";
    const WRITER: &'static str = "JP-REVIEW";
}

/// A review id.
///
/// The canonical form is `R-1h4k2m9`.
/// Input additionally accepts `R1h4k2m9`, the bare body, and any case.
pub type ReviewId = Id<ReviewKind>;

/// A comment, named by its review and its number there: `R-1h4k2m9#2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommentRef {
    pub review: ReviewId,
    pub number: u32,
}

impl fmt::Display for CommentRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.review, self.number)
    }
}

impl FromStr for CommentRef {
    type Err = String;

    /// Read `R-1h4k2m9#2`, with the review id in any spelling [`ReviewId`]
    /// accepts.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("`{text}` is not a comment (try R-1h4k2m9#2).");
        let (review, number) = text.trim().split_once('#').ok_or_else(invalid)?;

        Ok(Self {
            review: review.parse().map_err(|_| invalid())?,
            number: number
                .parse()
                .ok()
                .filter(|number| *number > 0)
                .ok_or_else(invalid)?,
        })
    }
}

impl Serialize for CommentRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// Conversation ID formatted as `jp-c17629384710`.
///
/// Parses `jp-c<id>`, `jp-c<id>-<global>`, or the bare numeric timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conversation(i64);

impl jp_id::Id for Conversation {
    fn variant() -> Variant {
        'c'.into()
    }

    fn target_id(&self) -> TargetId {
        self.0.to_string().into()
    }
}

impl fmt::Display for Conversation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        jp_id::Id::format_id(self, f)
    }
}

impl FromStr for Conversation {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        let number = match jp_id::parse::<Self>(text) {
            Ok(Parts { target_id, .. }) => target_id.parse::<i64>().ok(),
            Err(_) => text.parse::<i64>().ok(),
        };

        number
            .filter(|number| *number > 0)
            .map(Self)
            .ok_or_else(|| format!("`{text}` is not a conversation (try jp-c17629384710)."))
    }
}

#[cfg(test)]
#[path = "id_tests.rs"]
mod tests;
