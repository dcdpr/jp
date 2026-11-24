//! Core review data model and state transitions (`review.json`).
//!
//! [`Review`] represents the serialized state of a review: tracked file paths,
//! file versions, comments, replies, passes, and what the reviewer approved and
//! read ([`crate::progress`]).
//! All mutations validate invariants and return a [`Refusal`] on invalid state
//! transitions; disk I/O is handled by [`crate::store`] and
//! [`crate::snapshot`].

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
// `review.json` stores these as the HTTP API writes them.
pub use jp_review_api::{Draft, Resolution, Role, State, Status, Verdict};
use serde::{Deserialize, Serialize};

use crate::{
    anchor::{Anchor, Target},
    id::{CommentRef, ReviewId},
    pass::Passes,
    progress::Progress,
};

/// Current `review.json` schema version.
pub const FORMAT: u32 = 1;

/// Label assigned to the base version when a review is started with a `base`
/// command.
pub const BASE_LABEL: &str = "base";

/// Map from tracked file path to its SHA-256 blob hash in a version, or `None`
/// if the file was absent in that version.
pub type Files = BTreeMap<String, Option<String>>;

/// Serialized state of a review in `review.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub format: u32,

    /// Monotonically increasing counter incremented on each write.
    pub revision: u64,

    pub title: String,
    pub author: String,
    pub created_at: DateTime<Utc>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,

    /// Tracked file paths, relative to the workspace root.
    pub paths: Vec<String>,

    pub versions: Vec<Version>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<Comment>,

    /// Highest comment number allocated in this review, including withdrawn
    /// draft comments.
    #[serde(default)]
    pub last_comment: u32,

    #[serde(flatten)]
    pub passes: Passes,

    #[serde(flatten)]
    pub progress: Progress,
}

/// Snapshot of the review's tracked files at a point in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    pub number: u32,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,

    pub created_at: DateTime<Utc>,
    pub files: Files,
}

/// Comment anchored to the review, a file, or a range, along with its reply
/// thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub number: u32,
    pub role: Role,
    pub author: String,
    pub created_at: DateTime<Utc>,

    #[serde(flatten)]
    pub content: Content,

    /// Pass number in which this comment was first sent to the assistant (or
    /// opened by the assistant); `None` while the comment is a reviewer draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<u32>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replies: Vec<Reply>,

    /// Pending reviewer reply to be sent in the next pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_reply: Option<Reply>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<DateTime<Utc>>,
}

/// What a comment says, and what it is about.
///
/// An edit to a draft replaces it whole, once the result passes
/// [`Content::check`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub anchor: Anchor,

    /// Markdown; empty on a suggestion that needs no explanation.
    pub body: String,

    /// Text to replace the anchored range with, which makes the comment a
    /// suggestion; empty to suggest deleting the range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// A reply to a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply {
    pub role: Role,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub body: String,

    /// What the assistant made of a reviewer's comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,

    /// The pass that sent the reviewer's reply, or the one the assistant wrote
    /// its reply during.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<u32>,
}

/// A comment to add to a review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewComment {
    pub role: Role,
    pub author: String,
    pub created_at: DateTime<Utc>,
    pub content: Content,
}

/// What an edit changes; `None` leaves that part as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    pub anchor: Option<Anchor>,
    pub suggestion: Option<String>,
    pub body: Option<String>,
}

/// What a reply did to the comment's draft reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replied {
    Added,
    Replaced,
}

/// Why a review refused a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The review is closed.
    Closed,

    /// The comment with this number stands in the way, for the reason given.
    Comment(u32, Why),

    /// A comment or reply needs a body, unless the comment is a suggestion.
    EmptyBody,

    /// A suggestion replaces a range, and the comment is on none.
    SuggestionWithoutRange,

    /// An edit that changes nothing.
    NothingToChange,

    /// A pass with no comment waiting on the assistant, and no message.
    NothingToSend,

    /// The review runs its passes in this conversation, not the one named.
    Bound(String),

    /// The pass with this number has not ended.
    PassRunning(u32),

    /// An approval is of a range of a file, and the anchor names none.
    ApprovalWithoutRange,

    /// No approval carries this number.
    Approval(u32),
}

/// What about a comment stands in the way of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// No comment carries the number.
    Missing,

    /// The comment was withdrawn while a draft.
    Withdrawn,

    /// The comment is a draft, so it has nothing to reply to or resolve.
    Draft,

    /// The comment was sent and holds no draft reply, so it has nothing to edit
    /// or withdraw.
    Sent,

    /// The comment is resolved, so it takes no reply.
    Resolved,

    /// The comment holds a draft reply, which resolving it would strand.
    DraftReply,

    /// A draft reply has only a body to change.
    ReplyAnchor,

    /// The assistant spoke last in the comment, so a pass has nothing of it to
    /// send.
    Answered,

    /// A reply to the reviewer's comment needs a verdict.
    VerdictRequired,

    /// A reply to the assistant's own comment takes no verdict.
    VerdictRefused,
}

impl Refusal {
    /// The refusal as it reads to the user, naming comments in the review `id`.
    pub fn message(&self, id: ReviewId) -> String {
        match self {
            Self::Closed => format!("{id} is closed."),
            Self::Comment(number, why) => why.message(CommentRef {
                review: id,
                number: *number,
            }),
            Self::EmptyBody => "The body is empty.".to_owned(),
            Self::SuggestionWithoutRange => {
                "A suggestion replaces a range of a file, and the comment is on none.".to_owned()
            }
            Self::NothingToChange => "The edit changes nothing.".to_owned(),
            Self::NothingToSend => format!(
                "{id} has nothing to send: no draft, and no comment waiting on the assistant. \
                 Pass `-m` to send a message on its own."
            ),
            Self::Bound(conversation) => format!(
                "{id} runs its passes in {conversation}; leave out `--id` to continue there."
            ),
            Self::PassRunning(number) => format!("Pass {number} of {id} has not ended."),
            Self::ApprovalWithoutRange => {
                "An approval is of a range of a file: lines, or a span within them.".to_owned()
            }
            Self::Approval(number) => format!("{id} has no approval {number}."),
        }
    }
}

/// Why a change to a review didn't happen: the review refused it, or something
/// else went wrong, such as reading its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Refused(ReviewId, Refusal),
    Other(String),
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<Failure> for String {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Refused(id, refusal) => refusal.message(id),
            Failure::Other(message) => message,
        }
    }
}

impl Why {
    /// The refusal as it reads to the user, naming `comment`.
    fn message(self, comment: CommentRef) -> String {
        match self {
            Self::Missing => format!("No comment {comment}."),
            Self::Withdrawn => format!("{comment} was withdrawn."),
            Self::Draft => format!("{comment} is a draft, not yet sent."),
            Self::Sent => format!("{comment} has been sent, and holds no draft reply."),
            Self::Resolved => format!("{comment} is resolved."),
            Self::DraftReply => {
                format!("{comment} holds a draft reply; withdraw it before resolving the comment.")
            }
            Self::ReplyAnchor => {
                format!("{comment} holds a draft reply, which has only a body to change.")
            }
            Self::Answered => {
                format!("The assistant answered {comment} last; reply to it before sending it.")
            }
            Self::VerdictRequired => format!(
                "A reply to {comment}, the reviewer's comment, needs a verdict: accept, amend, \
                 decline, defer, or ask."
            ),
            Self::VerdictRefused => {
                format!("{comment} was opened by the assistant, so a reply to it takes no verdict.")
            }
        }
    }
}

impl Review {
    /// A review of `paths`, with no versions or comments yet.
    pub fn new(
        title: String,
        author: String,
        created_at: DateTime<Utc>,
        paths: Vec<String>,
    ) -> Self {
        Self {
            format: FORMAT,
            revision: 1,
            title,
            author,
            created_at,
            closed_at: None,
            paths,
            versions: vec![],
            comments: vec![],
            last_comment: 0,
            passes: Passes::default(),
            progress: Progress::default(),
        }
    }

    pub fn state(&self) -> State {
        match self.closed_at {
            Some(_) => State::Closed,
            None => State::Open,
        }
    }

    pub fn latest(&self) -> Option<&Version> {
        self.versions.last()
    }

    pub fn version(&self, number: u32) -> Option<&Version> {
        self.versions
            .iter()
            .find(|version| version.number == number)
    }

    /// The version labelled [`BASE_LABEL`], if the review has one.
    pub fn base(&self) -> Option<u32> {
        self.versions
            .iter()
            .find(|version| version.label.as_deref() == Some(BASE_LABEL))
            .map(|version| version.number)
    }

    /// The blob holding `path` in version `number`; `None` when there is no
    /// such version, or the file was absent from it.
    pub fn blob(&self, number: u32, path: &str) -> Option<&str> {
        self.version(number)?.files.get(path)?.as_deref()
    }

    /// Record `files` as the next version, returning its number.
    pub fn record(&mut self, files: Files, label: Option<&str>, at: DateTime<Utc>) -> u32 {
        let number = self.latest().map_or(1, |latest| latest.number + 1);
        self.versions.push(Version {
            number,
            label: label.map(ToOwned::to_owned),
            created_at: at,
            files,
        });

        number
    }

    /// Refuse any change once the review is closed.
    pub fn writable(&self) -> Result<(), Refusal> {
        match self.state() {
            State::Open => Ok(()),
            State::Closed => Err(Refusal::Closed),
        }
    }

    pub fn comment(&self, number: u32) -> Result<&Comment, Refusal> {
        let missing = missing(self.last_comment, number);
        self.comments
            .iter()
            .find(|comment| comment.number == number)
            .ok_or(missing)
    }

    pub(crate) fn comment_mut(&mut self, number: u32) -> Result<&mut Comment, Refusal> {
        let missing = missing(self.last_comment, number);
        self.comments
            .iter_mut()
            .find(|comment| comment.number == number)
            .ok_or(missing)
    }

    /// Add a comment, returning the number it takes.
    ///
    /// A reviewer's comment starts as a draft; an assistant's is open, and
    /// belongs to the pass running when it was opened.
    pub fn add_comment(&mut self, new: NewComment) -> Result<u32, Refusal> {
        self.writable()?;
        new.content.check()?;

        let pass = match new.role {
            Role::Reviewer => None,
            Role::Assistant => self.passes.running().map(|pass| pass.number),
        };
        let highest = self.comments.iter().map(|comment| comment.number).max();
        let number = self.last_comment.max(highest.unwrap_or(0)) + 1;
        self.last_comment = number;
        self.comments.push(Comment {
            number,
            role: new.role,
            author: new.author,
            created_at: new.created_at,
            content: new.content,
            pass,
            replies: vec![],
            draft_reply: None,
            resolved_at: None,
        });

        Ok(number)
    }

    /// Write the reviewer's draft reply on an open comment, replacing any it
    /// holds.
    pub fn reply(
        &mut self,
        number: u32,
        author: String,
        at: DateTime<Utc>,
        body: String,
    ) -> Result<Replied, Refusal> {
        self.writable()?;
        if body.trim().is_empty() {
            return Err(Refusal::EmptyBody);
        }

        let comment = self.comment_mut(number)?;
        comment.open()?;

        let replaced = comment.draft_reply.replace(Reply {
            role: Role::Reviewer,
            author,
            created_at: at,
            body,
            verdict: None,
            pass: None,
        });

        Ok(match replaced {
            Some(_) => Replied::Replaced,
            None => Replied::Added,
        })
    }

    /// Record the assistant's reply to a sent comment, as part of the pass
    /// running, if one is.
    ///
    /// A reply to the reviewer's comment carries a verdict, and one to the
    /// assistant's own comment carries none.
    pub fn answer(&mut self, number: u32, mut reply: Reply) -> Result<(), Refusal> {
        self.writable()?;
        if reply.body.trim().is_empty() {
            return Err(Refusal::EmptyBody);
        }
        reply.pass = self.passes.running().map(|pass| pass.number);

        let comment = self.comment_mut(number)?;
        comment.open()?;
        match (comment.role, reply.verdict) {
            (Role::Reviewer, None) => return Err(Refusal::Comment(number, Why::VerdictRequired)),
            (Role::Assistant, Some(_)) => {
                return Err(Refusal::Comment(number, Why::VerdictRefused));
            }
            _ => {}
        }
        comment.replies.push(reply);

        Ok(())
    }

    /// Change a draft comment, or the body of the draft reply on a sent one.
    pub fn edit(&mut self, number: u32, edit: Edit) -> Result<Draft, Refusal> {
        self.writable()?;
        if edit == Edit::default() {
            return Err(Refusal::NothingToChange);
        }

        let comment = self.comment_mut(number)?;
        if comment.status() == Status::Draft {
            let content = comment.content.edited(edit);
            content.check()?;
            comment.content = content;
            return Ok(Draft::Comment);
        }

        let reply = comment
            .draft_reply
            .as_mut()
            .ok_or(Refusal::Comment(number, Why::Sent))?;
        if edit.anchor.is_some() || edit.suggestion.is_some() {
            return Err(Refusal::Comment(number, Why::ReplyAnchor));
        }
        let body = edit.body.unwrap_or_default();
        if body.trim().is_empty() {
            return Err(Refusal::EmptyBody);
        }
        reply.body = body;

        Ok(Draft::Reply)
    }

    /// Withdraw a draft comment, retiring its number, or the draft reply on a
    /// sent one.
    pub fn withdraw(&mut self, number: u32) -> Result<Draft, Refusal> {
        self.writable()?;
        let missing = missing(self.last_comment, number);
        let index = self
            .comments
            .iter()
            .position(|comment| comment.number == number)
            .ok_or(missing)?;

        if self.comments[index].status() == Status::Draft {
            self.comments.remove(index);
            return Ok(Draft::Comment);
        }

        self.comments[index]
            .draft_reply
            .take()
            .map(|_| Draft::Reply)
            .ok_or(Refusal::Comment(number, Why::Sent))
    }

    /// Mark an open comment resolved.
    pub fn resolve(&mut self, number: u32, at: DateTime<Utc>) -> Result<Resolution, Refusal> {
        self.writable()?;
        let comment = self.comment_mut(number)?;

        match comment.status() {
            Status::Draft => Err(Refusal::Comment(number, Why::Draft)),
            Status::Resolved => Ok(Resolution::AlreadyResolved),
            Status::Open if comment.draft_reply.is_some() => {
                Err(Refusal::Comment(number, Why::DraftReply))
            }
            Status::Open => {
                comment.resolved_at = Some(at);
                Ok(Resolution::Resolved)
            }
        }
    }

    /// The open comments whose last reply is the assistant accepting them, with
    /// no draft reply waiting.
    pub fn accepted(&self) -> Vec<u32> {
        self.comments
            .iter()
            .filter(|comment| comment.status() == Status::Open && comment.draft_reply.is_none())
            .filter(|comment| {
                comment.replies.last().is_some_and(|reply| {
                    reply.role == Role::Assistant && reply.verdict == Some(Verdict::Accept)
                })
            })
            .map(|comment| comment.number)
            .collect()
    }

    /// Close the review, returning `false` when it was closed already.
    ///
    /// Refused while a pass runs, since the assistant answering it could no
    /// longer write its replies.
    pub fn close(&mut self, at: DateTime<Utc>) -> Result<bool, Refusal> {
        if self.closed_at.is_some() {
            return Ok(false);
        }
        if let Some(running) = self.passes.running() {
            return Err(Refusal::PassRunning(running.number));
        }

        self.closed_at = Some(at);
        Ok(true)
    }

    /// Open a closed review again, returning `false` when it was open already.
    pub fn reopen(&mut self) -> bool {
        self.closed_at.take().is_some()
    }
}

/// Why no comment carries `number` in a review whose highest comment number
/// allocated is `last`: it was withdrawn when a comment has taken it before.
fn missing(last: u32, number: u32) -> Refusal {
    let why = if (1..=last).contains(&number) {
        Why::Withdrawn
    } else {
        Why::Missing
    };

    Refusal::Comment(number, why)
}

impl Comment {
    pub fn status(&self) -> Status {
        if self.resolved_at.is_some() {
            Status::Resolved
        } else if self.role == Role::Reviewer && self.pass.is_none() {
            Status::Draft
        } else {
            Status::Open
        }
    }

    /// Refuse a reply to a comment that isn't open.
    fn open(&self) -> Result<(), Refusal> {
        match self.status() {
            Status::Open => Ok(()),
            Status::Draft => Err(Refusal::Comment(self.number, Why::Draft)),
            Status::Resolved => Err(Refusal::Comment(self.number, Why::Resolved)),
        }
    }

    /// Open a resolved comment again, returning `false` when it was open
    /// already.
    ///
    /// A draft was never sent, so it was never resolved either.
    pub fn reopen(&mut self) -> Result<bool, Refusal> {
        match self.status() {
            Status::Draft => Err(Refusal::Comment(self.number, Why::Draft)),
            Status::Open => Ok(false),
            Status::Resolved => {
                self.resolved_at = None;
                Ok(true)
            }
        }
    }

    /// Whether the comment holds something the assistant hasn't answered: it is
    /// a draft, holds a draft reply, or the reviewer wrote in it last.
    pub fn awaits_assistant(&self) -> bool {
        let last = self.replies.last().map_or(self.role, |reply| reply.role);

        match self.status() {
            Status::Resolved => false,
            Status::Draft => true,
            Status::Open => self.draft_reply.is_some() || last == Role::Reviewer,
        }
    }

    /// What the reviewer wrote in the comment since the assistant last did:
    /// whether that includes the comment's own body, and the replies.
    pub fn unanswered(&self) -> (bool, &[Reply]) {
        let answered = self
            .replies
            .iter()
            .rposition(|reply| reply.role == Role::Assistant);

        match answered {
            Some(index) => (false, &self.replies[index + 1..]),
            None => (self.role == Role::Reviewer, &self.replies),
        }
    }
}

impl Content {
    /// Refuse what no comment can say: a comment needs a body or a suggestion,
    /// and a suggestion needs a range to replace.
    pub fn check(&self) -> Result<(), Refusal> {
        let range = matches!(self.anchor.target, Target::Range(..));
        match &self.suggestion {
            Some(_) if !range => Err(Refusal::SuggestionWithoutRange),
            None if self.body.trim().is_empty() => Err(Refusal::EmptyBody),
            _ => Ok(()),
        }
    }

    /// This content, with what `edit` names changed.
    fn edited(&self, edit: Edit) -> Self {
        Self {
            anchor: edit.anchor.unwrap_or_else(|| self.anchor.clone()),
            body: edit.body.unwrap_or_else(|| self.body.clone()),
            suggestion: edit.suggestion.or_else(|| self.suggestion.clone()),
        }
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
