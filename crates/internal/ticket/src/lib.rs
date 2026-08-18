//! In-repo tickets: work items tracked as markdown files under `docs/ticket/`.
//!
//! A ticket is a single file, `docs/ticket/<id>-slug.md`, holding a metadata
//! block, a description, and an append-only list of comments:
//!
//! ```markdown
//! # Tool call header misaligned
//!
//! - **Status**: Todo
//! - **Kind**: Bug
//! - **Authors**: John Doe
//! - **Date**: 2026-08-05
//!
//! The header renders one column left of the body below 80 columns.
//!
//! ## Comments
//!
//! -----
//!
//! - **From**: john
//! - **Date**: 2026-08-05T14:03:11Z
//!
//! Reproduced at 72 columns. Not at 80.
//! ```
//!
//! A comment starts at a line of five or more dashes followed by a blank line
//! and a metadata block carrying both `From` and `Date`; everything before the
//! first such boundary is the description.
//! Each comment opens with its own separator, so writing one appends the
//! separator, the metadata, and the body, leaving everything above untouched.
//!
//! [`id`] defines the identifier, [`parse`] reads a document, [`render`] writes
//! one, [`store`] holds the file operations (id allocation, create, comment,
//! close, list, import), and [`import`] carries the rules for content that
//! comes from upstream.
//!
//! The id is in the filename and nowhere else, so renaming a ticket is a rename
//! and there is no second copy to keep in step.
//!
//! The format is specified in `docs/rfd/100-in-repo-ticket-tracking.md`.

use std::{fmt, str::FromStr};

use serde::Serialize;

pub mod id;
pub mod import;
pub mod parse;
pub mod render;
pub mod store;

pub use id::TicketId;

/// Where a ticket sits on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Status {
    Todo,
    #[serde(rename = "In Progress")]
    InProgress,
    Done,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Todo => "Todo",
            Self::InProgress => "In Progress",
            Self::Done => "Done",
        })
    }
}

impl FromStr for Status {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_'], " ")
            .as_str()
        {
            "todo" => Ok(Self::Todo),
            "in progress" | "inprogress" => Ok(Self::InProgress),
            "done" => Ok(Self::Done),
            _ => Err(ParseError::InvalidValue {
                field: "Status",
                value: s.to_owned(),
            }),
        }
    }
}

/// What kind of work a ticket describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Kind {
    Bug,
    Feature,
    Chore,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Bug => "Bug",
            Self::Feature => "Feature",
            Self::Chore => "Chore",
        })
    }
}

impl FromStr for Kind {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bug" => Ok(Self::Bug),
            "feature" => Ok(Self::Feature),
            "chore" => Ok(Self::Chore),
            _ => Err(ParseError::InvalidValue {
                field: "Kind",
                value: s.to_owned(),
            }),
        }
    }
}

/// A ticket's metadata block.
///
/// The repository owns every field here: a GitHub import replaces the title,
/// description, and comments, and leaves this block alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Metadata {
    pub status: Status,
    pub kind: Kind,
    pub authors: String,
    pub date: String,
    pub blocked_by: Option<String>,
    pub implements: Option<String>,
    pub promoted_to: Option<String>,
    pub github: Option<String>,
}

/// One comment on a ticket.
///
/// `from` is a short handle (`john`, `jp`); imported GitHub comments use
/// `gh:username`.
/// `re` holds the comment being replied to, as `#1`, where the number is the
/// 1-based position of that comment in the file.
/// A reply always targets a comment on the same ticket, so it names no id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Comment {
    pub from: String,
    pub date: String,
    pub re: Option<String>,
    pub body: String,
}

/// A parsed ticket document.
///
/// Carries no id: that is the filename's, and a caller who read the file has it
/// already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ticket {
    pub title: String,
    pub metadata: Metadata,
    pub description: String,
    pub comments: Vec<Comment>,
}

/// A document that doesn't hold a well-formed ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The document doesn't open with a `# Title` heading.
    MissingTitle,
    /// The title heading isn't followed by a metadata block.
    MissingMetadata,
    /// A required metadata field is absent.
    MissingField(&'static str),
    /// A metadata field holds a value the format doesn't define.
    InvalidValue { field: &'static str, value: String },
    /// A string that isn't a ticket id.
    Id(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingTitle => f.write_str("Ticket does not open with a `# Title` heading."),
            Self::MissingMetadata => {
                f.write_str("Ticket title is not followed by a metadata block.")
            }
            Self::MissingField(field) => write!(f, "Ticket is missing the `{field}` field."),
            Self::InvalidValue { field, value } => {
                write!(f, "`{value}` is not a valid `{field}` value.")
            }
            Self::Id(value) => write!(f, "`{value}` is not a ticket id (try T-02wt0kx)."),
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
