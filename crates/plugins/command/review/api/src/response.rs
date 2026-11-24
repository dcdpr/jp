//! What the server answers with, and the vocabulary a review is described in.
//!
//! An answer the server leaves a field out of reads back as that field's
//! default, and a field this crate doesn't know is ignored, so a client keeps
//! reading what a newer server adds.

use std::{collections::BTreeMap, fmt};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What `GET /api/reviews` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// The name of the workspace the server serves: its root directory's.
    pub workspace: String,

    /// Every review that can be read, oldest first.
    pub reviews: Vec<Summary>,

    /// The reviews the store holds and can't read, and why.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable: Vec<Unreadable>,
}

/// A review the store holds and can't read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unreadable {
    pub id: String,
    pub error: String,
}

/// What every record of a review opens with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// The review: `R-1h4k2m9`.
    pub id: String,
    pub title: String,
    pub status: State,
    pub author: String,
    pub created_at: DateTime<Utc>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,

    /// The files the review tracks, relative to the workspace root.
    pub files: Vec<String>,
}

/// A review in a listing, which is also what starting or closing one answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    #[serde(flatten)]
    pub head: Head,

    /// The number of the latest version.
    pub latest: u32,

    /// The version labelled `base`, if there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<u32>,

    pub comments: Counts,

    /// The number of the pass running on the review, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<u32>,
}

/// How many of a review's comments stand where.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub draft: usize,
    pub open: usize,
    pub resolved: usize,
}

/// What `GET /api/reviews/{id}` answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailReply {
    /// The name of the workspace the server serves: its root directory's.
    pub workspace: String,

    /// The review's revision, which goes up with every change to it.
    pub revision: u64,

    /// The pass running on the review, if one is.
    pub running: Option<Running>,

    /// Whether a file the review tracks differs in the working tree from the
    /// latest version; always `false` while a pass runs, whose end records the
    /// files.
    ///
    /// A tracked file that can't be read counts as differing.
    pub working_tree_changed: bool,

    /// The review; absent when the caller holds its revision already.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<Detail>,
}

/// The pass running on a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Running {
    pub number: u32,

    /// The conversation its turn runs in, once the host has named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,

    /// Whether this server runs it, which is what lets `POST
    /// /api/reviews/{id}/stop` stop it.
    pub stoppable: bool,
}

/// A review, with its comments placed in one version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detail {
    #[serde(flatten)]
    pub head: Head,

    pub revision: u64,

    /// The version the comments are placed in.
    pub version: u32,

    pub versions: Vec<Version>,

    /// The conversation the review's passes run in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub passes: Vec<Pass>,

    pub comments: Vec<Comment>,
}

/// A snapshot of the review's files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub number: u32,

    /// `base` for the version a base command printed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    pub created_at: DateTime<Utc>,

    /// Each tracked file, and the SHA-256 of what it held; `null` for a file
    /// absent from the version.
    pub files: BTreeMap<String, Option<String>>,
}

/// A batch of comments and replies sent to the assistant, and how the turn
/// answering them ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pass {
    pub number: u32,
    pub author: String,
    pub created_at: DateTime<Utc>,

    /// The version the pass was sent against.
    pub version: u32,

    /// The comments the pass sent, by number.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<u32>,

    /// The reviewer's message to the assistant beside the comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// The conversation the pass's turn ran in, once the host named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,

    /// How the pass ended; absent while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
}

/// How a pass ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ended {
    pub outcome: Outcome,
    pub at: DateTime<Utc>,

    /// The version recorded when the pass ended, which holds the assistant's
    /// edits; absent when the files couldn't be recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,

    /// What went wrong, for a pass that failed or lost its host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// The assistant's closing message, in Markdown: the last text it wrote in
    /// the pass's turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// A comment, with its anchor placed in the version shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    /// The comment: `R-1h4k2m9#2`.
    pub id: String,
    pub status: Status,
    pub role: Role,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub anchor: AnchorView,

    /// The text the anchor covers, in the version it was written against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,

    /// The comment, in Markdown; empty on a suggestion that needs no
    /// explanation.
    pub body: String,

    /// `body` rendered as HTML, raw HTML in it escaped.
    pub body_html: String,

    /// The text the comment suggests replacing its range with; empty to suggest
    /// deleting it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,

    /// The reviewer's reply, held until the next pass sends it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_reply: Option<Reply>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

/// A reply to a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub role: Role,
    pub author: String,
    pub created_at: DateTime<Utc>,

    /// The reply, in Markdown.
    pub body: String,

    /// `body` rendered as HTML, raw HTML in it escaped.
    pub body_html: String,

    /// What the assistant made of a reviewer's comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,

    /// The pass that sent the reviewer's reply, or the one the assistant wrote
    /// its reply during.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<u32>,
}

/// What a comment is on, as written, and where that falls in the version shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorView {
    /// The version the comment was written against.
    pub version: u32,

    /// The file; absent for a comment on the review as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// The range in `path` as of `version`; absent for a comment on the whole
    /// file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,

    /// Where the anchor falls in the version shown; absent when it is outdated
    /// there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected: Option<Placed>,

    pub outdated: bool,

    /// For an anchor outdated in the version shown, the latest version before
    /// it that still holds the anchor's text unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved: Option<Placed>,
}

/// Where an anchor falls in a version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    pub version: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,

    /// The half-open UTF-8 byte range `range` covers in the version.
    /// A range of whole lines covers them without the last one's line ending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<[usize; 2]>,
}

/// What the file endpoint answers: a file as one version holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileReply {
    pub review: String,
    pub version: u32,
    pub path: String,
    pub source: String,

    /// The file rendered, when asked for: Markdown as Markdown, and any other
    /// file as preformatted text.
    /// Every run of text in it carries the byte range of the source it came
    /// from, in `data-s` and `data-e`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

/// What changed in a review's files from one version to another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffView {
    pub review: String,
    pub from: u32,
    pub to: u32,

    /// The files that changed; the others are left out.
    pub files: Vec<FileDiff>,
}

/// How one file changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub change: Change,

    /// The unified diff hunks, without file headers.
    pub diff: String,

    /// The half-open byte range, in the `to` version, of each line added or
    /// modified, without its line ending.
    #[serde(default)]
    pub added: Vec<[usize; 2]>,

    /// The byte offset, in the `to` version, where each run of removed lines
    /// was: the start of the line after them, or the end of the file.
    #[serde(default)]
    pub removed: Vec<usize>,
}

/// What withdrawing a draft did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Withdrawal {
    /// The comment: `R-1h4k2m9#2`.
    pub id: String,
    pub withdrawn: Draft,
}

/// What resolving a comment did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolved {
    /// The comment: `R-1h4k2m9#2`.
    pub id: String,
    pub outcome: Resolution,
}

/// What `POST /api/reviews/{id}/passes` answers: the pass as recorded, once its
/// turn has started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
    pub review: String,

    #[serde(flatten)]
    pub pass: Pass,
}

/// What `POST /api/reviews/{id}/stop` answers when it asked a pass to stop.
///
/// How the pass ended is recorded once its turn has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stopping {
    pub review: String,

    /// The pass asked to stop.
    pub number: u32,

    pub stopping: bool,
}

/// What `POST /api/reviews/{id}/versions` answers: the version holding the
/// working tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recorded {
    pub review: String,
    pub version: u32,

    /// Whether the request recorded `version`; `false` when the working tree
    /// held what the latest version does already.
    pub recorded: bool,
}

/// The body of every error answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: String,
}

/// Whether a review takes changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Closed,
}

/// Where a comment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Written by the reviewer, and not yet sent to the assistant.
    Draft,
    /// Sent, and waiting on one side or the other.
    Open,
    /// Closed by the reviewer.
    Resolved,
}

/// Which side of a review wrote something.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Reviewer,
    Assistant,
}

/// The assistant's disposition on a reviewer's comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Agrees, and makes the requested change.
    Accept,
    /// Agrees with the issue, and makes a different change.
    Amend,
    /// Makes no change, and says why.
    Decline,
    /// Agrees, but leaves the point for later, saying where it was tracked.
    Defer,
    /// Asks a question before changing anything.
    Ask,
}

/// How the turn of a pass ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The turn finished.
    Completed,

    /// The turn was stopped or aborted.
    Stopped,

    /// The host couldn't run the turn, or the turn failed.
    Failed,

    /// The process running the pass exited, or lost its host, before it learned
    /// how the turn ended.
    Interrupted,
}

/// What an edit or a withdrawal applied to: a draft comment, or a comment's
/// draft reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Draft {
    Comment,
    Reply,
}

/// What resolving a comment did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Resolved,
    AlreadyResolved,
}

/// How one file changed from one version to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    Added,
    Deleted,
    Modified,
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Open => "open",
            Self::Closed => "closed",
        })
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Draft => "draft",
            Self::Open => "open",
            Self::Resolved => "resolved",
        })
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Accept => "accept",
            Self::Amend => "amend",
            Self::Decline => "decline",
            Self::Defer => "defer",
            Self::Ask => "ask",
        })
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Completed => "completed",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        })
    }
}

#[cfg(test)]
#[path = "response_tests.rs"]
mod tests;
