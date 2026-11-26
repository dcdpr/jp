//! Review passes: batches of reviewer comments and replies sent to the
//! assistant, and the turns that answer them.
//!
//! [`Passes`] is flattened into [`Review`] in `review.json`:
//!
//! ```json
//! "conversation": "jp-c17629384710",
//! "passes": [
//!   {
//!     "number": 1,
//!     "author": "Jane Doe <jane@example.com>",
//!     "created_at": "2026-11-17T10:00:00Z",
//!     "version": 2,
//!     "comments": [1, 2],
//!     "conversation": "jp-c17629384710",
//!     "ended": {
//!       "outcome": "completed",
//!       "at": "2026-11-17T10:04:00Z",
//!       "version": 3,
//!       "answer": "Both comments are addressed."
//!     }
//!   }
//! ]
//! ```
//!
//! A pass with `ended == None` is either currently running or was left behind
//! by a process that exited before recording its outcome; [`crate::store`]
//! distinguishes the two using the review's `.pass` advisory lock.

use std::fmt;

use chrono::{DateTime, Utc};
// `review.json` stores it as the HTTP API writes it.
pub use jp_review_api::Outcome;
use serde::{Deserialize, Serialize};

use crate::model::{Refusal, Review, Role, Why};

/// A review's passes, and the conversation they run in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passes {
    /// The conversation the review's passes run in, once a pass established it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,

    /// Every pass, in the order they were sent.
    #[serde(default, rename = "passes", skip_serializing_if = "Vec::is_empty")]
    pub list: Vec<Pass>,
}

/// One batch of comments and replies sent to the assistant, and how the turn
/// answering them ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pass {
    pub number: u32,
    pub author: String,
    pub created_at: DateTime<Utc>,

    /// Version against which the pass was submitted.
    pub version: u32,

    /// Comment numbers included in this pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<u32>,

    /// Optional reviewer message sent alongside the comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Conversation ID in which the pass turn ran (`None` until the host
    /// creates the conversation on the first pass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,

    /// Pass outcome; `None` while the pass is running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Ended>,
}

/// How a pass ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ended {
    pub outcome: Outcome,
    pub at: DateTime<Utc>,

    /// The latest version when the pass recorded how it ended, which holds the
    /// assistant's edits; absent for an interrupted pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,

    /// What went wrong, for a pass that failed or lost its host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// The assistant's closing message: the last text it wrote in the turn, as
    /// the host reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
}

/// A pass named on the command line: its number, or the latest one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassRef {
    Last,
    Number(u32),
}

impl PassRef {
    /// Read `last`, or a pass's number, such as `2`.
    ///
    /// # Errors
    ///
    /// What is wrong with `text`, as a fragment for a message that names where
    /// it came from.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("last") {
            return Ok(Self::Last);
        }

        text.parse()
            .map(Self::Number)
            .map_err(|_| "write a pass's number, such as `2`, or `last`".to_owned())
    }
}

/// Why what a pass changed in the files isn't known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unspanned {
    /// The review has no passes.
    NoPasses,

    /// The review has no pass with this number.
    Missing(u32),

    /// The pass is running, so its edits aren't recorded yet.
    Running(u32),

    /// The pass ended without recording the files, as an interrupted one does.
    Unrecorded(u32),
}

impl Unspanned {
    /// What to tell the user, about the review `id`.
    pub fn message(self, id: impl fmt::Display) -> String {
        match self {
            Self::NoPasses => format!("{id} has no passes."),
            Self::Missing(number) => format!("{id} has no pass {number}."),
            Self::Running(number) => format!(
                "Pass {number} of {id} is still running; what it changes is recorded when it ends."
            ),
            Self::Unrecorded(number) => format!(
                "Pass {number} of {id} ended without recording the files, so what it changed \
                 isn't known."
            ),
        }
    }
}

/// What a pass message carries of the review's files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carry {
    /// Each file whole, which the first pass in a conversation sends.
    Files,

    /// What changed since pass `pass` was sent, with the files at `version`,
    /// the version that pass was sent with.
    ///
    /// The assistant's own edits during pass `pass` are in the diff too: the
    /// version recorded when it ended also holds edits made outside the turn
    /// while it ran, which the assistant never read.
    Changes { pass: u32, version: u32 },
}

/// A pass to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPass {
    pub author: String,
    pub created_at: DateTime<Utc>,

    /// The version holding the files the pass is sent against.
    pub version: u32,

    /// The comments to send, by number; every comment waiting on the assistant
    /// when empty.
    pub only: Vec<u32>,

    pub message: Option<String>,

    /// The conversation the pass is asked to run in; the review's own, or a new
    /// one when the review has none, when absent.
    pub conversation: Option<String>,
}

impl Passes {
    /// The pass that hasn't ended, if one hasn't.
    pub fn running(&self) -> Option<&Pass> {
        self.list.iter().find(|pass| pass.ended.is_none())
    }

    pub fn get(&self, number: u32) -> Option<&Pass> {
        self.list.iter().find(|pass| pass.number == number)
    }

    /// The number of the pass `which` names, and the versions it changed the
    /// files between: the one it was sent against, and the one recorded when it
    /// ended.
    ///
    /// The second holds whatever changed while the turn ran, the reviewer's own
    /// edits included.
    pub fn span(&self, which: PassRef) -> Result<(u32, [u32; 2]), Unspanned> {
        let pass = match which {
            PassRef::Last => self.list.last().ok_or(Unspanned::NoPasses)?,
            PassRef::Number(number) => self.get(number).ok_or(Unspanned::Missing(number))?,
        };
        let ended = pass.ended.as_ref().ok_or(Unspanned::Running(pass.number))?;
        let to = ended.version.ok_or(Unspanned::Unrecorded(pass.number))?;

        Ok((pass.number, [pass.version, to]))
    }

    /// Record every pass without an outcome as interrupted, returning their
    /// numbers.
    pub fn interrupt(&mut self, at: DateTime<Utc>) -> Vec<u32> {
        let mut interrupted = vec![];
        for pass in self.list.iter_mut().filter(|pass| pass.ended.is_none()) {
            pass.ended = Some(Ended {
                outcome: Outcome::Interrupted,
                at,
                version: None,
                error: None,
                answer: None,
            });
            interrupted.push(pass.number);
        }

        interrupted
    }

    /// Record that pass `number` runs in `conversation`, which the host created
    /// for it, and the review's passes with it.
    pub fn created(&mut self, number: u32, conversation: &str) {
        if let Some(pass) = self.list.iter_mut().find(|pass| pass.number == number) {
            pass.conversation = Some(conversation.to_owned());
        }
        self.conversation = Some(conversation.to_owned());
    }

    /// Record how pass `number` ended.
    ///
    /// A pass whose turn ran binds the review to the conversation it ran in,
    /// when the review has none yet.
    pub fn end(&mut self, number: u32, ended: Ended) {
        let ran = ended.ran();
        let Some(pass) = self.list.iter_mut().find(|pass| pass.number == number) else {
            return;
        };
        pass.ended = Some(ended);

        if ran && self.conversation.is_none() {
            self.conversation = pass.conversation.clone();
        }
    }

    /// What the message of pass `number` carries of the files.
    ///
    /// The files whole, unless the turn of an earlier pass ran in the same
    /// conversation; then what changed since the latest such pass was sent.
    pub fn carry(&self, number: u32) -> Carry {
        let Some(conversation) = self
            .get(number)
            .and_then(|pass| pass.conversation.as_deref())
        else {
            return Carry::Files;
        };

        self.list
            .iter()
            .rev()
            .filter(|pass| pass.number < number)
            .filter(|pass| pass.conversation.as_deref() == Some(conversation))
            .find(|pass| pass.ended.as_ref().is_some_and(Ended::ran))
            .map_or(Carry::Files, |pass| Carry::Changes {
                pass: pass.number,
                version: pass.version,
            })
    }

    /// Whether a tool call from `conversation` belongs to the review: the
    /// review's passes run there, or its running pass does.
    pub fn admits(&self, conversation: &str) -> bool {
        let running = self.running().and_then(|pass| pass.conversation.as_deref());

        !conversation.is_empty()
            && (self.conversation.as_deref() == Some(conversation) || running == Some(conversation))
    }

    /// The conversation a pass asked to run in `named` runs in: the review's
    /// own when unnamed, and never another than the review's.
    fn conversation_for(&self, named: Option<String>) -> Result<Option<String>, Refusal> {
        match (named, &self.conversation) {
            (Some(named), Some(bound)) if named != *bound => Err(Refusal::Bound(bound.clone())),
            (named, bound) => Ok(named.or_else(|| bound.clone())),
        }
    }
}

impl Ended {
    /// Whether the pass's turn ran, so the assistant read its message.
    fn ran(&self) -> bool {
        matches!(self.outcome, Outcome::Completed | Outcome::Stopped)
    }
}

impl Review {
    /// The comments a pass sends, by number: the ones `only` names, or every
    /// comment waiting on the assistant.
    ///
    /// A comment `only` names has to be waiting on the assistant.
    pub fn pending(&self, only: &[u32]) -> Result<Vec<u32>, Refusal> {
        if only.is_empty() {
            return Ok(self
                .comments
                .iter()
                .filter(|comment| comment.awaits_assistant())
                .map(|comment| comment.number)
                .collect());
        }

        let mut numbers = vec![];
        for &number in only {
            let comment = self.comment(number)?;
            if comment.resolved_at.is_some() {
                return Err(Refusal::Comment(number, Why::Resolved));
            }
            if !comment.awaits_assistant() {
                return Err(Refusal::Comment(number, Why::Answered));
            }
            if !numbers.contains(&number) {
                numbers.push(number);
            }
        }
        numbers.sort_unstable();

        Ok(numbers)
    }

    /// Record a pass, returning its number.
    ///
    /// Each comment the pass sends stops being a draft, and its draft reply
    /// joins its replies.
    /// A pass needs a comment to send or a message, and runs only once the
    /// review's earlier passes have ended.
    pub fn submit(&mut self, new: NewPass) -> Result<u32, Refusal> {
        self.writable()?;
        if let Some(running) = self.passes.running() {
            return Err(Refusal::PassRunning(running.number));
        }
        let conversation = self.passes.conversation_for(new.conversation)?;
        let comments = self.pending(&new.only)?;
        if comments.is_empty() && new.message.is_none() {
            return Err(Refusal::NothingToSend);
        }

        let number = self.passes.list.last().map_or(1, |pass| pass.number + 1);
        for &sent in &comments {
            let comment = self.comment_mut(sent)?;
            if comment.role == Role::Reviewer {
                comment.pass.get_or_insert(number);
            }
            if let Some(mut reply) = comment.draft_reply.take() {
                reply.pass = Some(number);
                comment.replies.push(reply);
            }
        }
        self.passes.list.push(Pass {
            number,
            author: new.author,
            created_at: new.created_at,
            version: new.version,
            comments,
            message: new.message,
            conversation,
            ended: None,
        });

        Ok(number)
    }
}

#[cfg(test)]
#[path = "pass_tests.rs"]
mod tests;
