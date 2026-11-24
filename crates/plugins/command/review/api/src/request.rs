//! What the server takes: request bodies, and the query parameters of its `GET`
//! endpoints.
//!
//! A body naming a field the endpoint doesn't take is refused.

use serde::{Deserialize, Serialize};

use crate::response::Draft;

/// The body of `POST /api/reviews`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewReview {
    /// Files to review, relative to the workspace root.
    pub files: Vec<String>,

    /// What to call the review; its files when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// What a comment is on, in a request.
///
/// Without `path`, the comment is on the review as a whole; with `path` and no
/// range, on the whole file.
/// A range is given as `range` or as `bytes`, never both, and needs `path` and
/// `version`: the file it is in, and the version the caller read it in.
///
/// It is also the body of `POST /api/reviews/{id}/approvals`, which approves
/// the range, and of `PUT /api/reviews/{id}/read`, which says the file was read
/// through the last line of the range; both need a range.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorRequest {
    /// The version `path` and the range are read in; the latest when absent,
    /// which only a request without a range may leave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Lines, `42` or `42-45`, or a span from a line and column to another,
    /// `42:5-43:18`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,

    /// A half-open UTF-8 byte range `[start, end)` within the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<[usize; 2]>,
}

/// The body of `POST /api/reviews/{id}/comments`: a draft comment, or a
/// suggestion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewCommentRequest {
    /// What the comment is on; the review as a whole when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorRequest>,

    /// The comment, in Markdown; a suggestion may leave it empty.
    #[serde(default)]
    pub body: String,

    /// Text to replace the anchored range with, which makes the comment a
    /// suggestion; empty to suggest deleting the range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// The body of `PATCH /api/reviews/{id}/comments/{n}`, which changes a draft
/// comment, or the body of the draft reply on a sent one.
///
/// What is absent is left as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorRequest>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,

    /// The draft the edit was written for; the edit is refused once the comment
    /// holds another.
    /// Whichever the comment holds when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<Draft>,
}

/// The body of `POST /api/reviews/{id}/comments/{n}/replies`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplyRequest {
    /// The reply, in Markdown.
    pub body: String,
}

/// The body of `POST /api/reviews/{id}/passes`, which may be left out.
///
/// A pass runs under the configurations `plugins.command.review.options.cfg`
/// names, and a request can't add to them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PassRequest {
    /// The comments to send, by number; every comment waiting on the assistant
    /// when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<u32>,

    /// A message to the assistant beside the comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// The conversation to run the pass in, `jp-c17629384710`; the review's
    /// own, or a new one for its first pass, when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}

/// The body of `POST /api/reviews/{id}/stop`, which may be left out.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopRequest {
    /// The pass to stop; nothing is stopped when another pass is running.
    /// Whichever pass this server runs when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<u32>,
}

/// The query of `GET /api/reviews/{id}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailQuery {
    /// The revision the caller holds; the review is left out of the answer
    /// unless it has changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,

    /// The version to place the comments in; the latest when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
}

/// The query of `GET /api/reviews/{id}/versions/{n}/file`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileQuery {
    /// The file, as the review tracks it.
    pub path: String,

    #[serde(default)]
    pub format: Format,
}

/// How the file endpoint answers with a file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Its source alone.
    #[default]
    Text,

    /// Its source, and the file rendered as HTML.
    Html,
}

/// The query of `DELETE /api/reviews/{id}/read`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadQuery {
    /// The file, as the review tracks it.
    pub path: String,
}

/// The query of `GET /api/reviews/{id}/diff`.
///
/// Names either a pass, or the versions to compare.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffQuery {
    /// The version to compare from; the one before `to` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<u32>,

    /// The version to compare to; the latest when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<u32>,

    /// The pass whose changes to show: from the version it was sent against to
    /// the one recorded when it ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<u32>,

    /// The one file to compare; every file the review tracks when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
