//! What the composer holds: a comment, an edit, a reply, or a pass message
//! being written, and the request that saves it.
//!
//! A draft names everything its request needs (the file, the version, the
//! bytes, the comment) from the moment the composer opens, so one restored from
//! the browser's local storage after iOS reloads a tab saves against what it
//! was written for.

use jp_review_api::{
    AnchorRequest, Comment, EditRequest, NewCommentRequest, PassRequest, ReplyRequest,
};
use serde::{Deserialize, Serialize};

use crate::{
    actions,
    client::{self, Call},
};

/// A composer's contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub kind: Kind,

    /// The comment, reply, or message, in Markdown.
    #[serde(default)]
    pub body: String,

    /// The text to replace a range with, which makes a comment a suggestion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

/// What a draft is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A new comment on `on`.
    Comment { on: Target },

    /// Changes to the draft comment `number`, whose range held `excerpt` when
    /// it was written; `None` for a comment on a whole file or the review,
    /// which can't be a suggestion.
    Edit {
        number: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        excerpt: Option<String>,
    },

    /// The reviewer's draft reply on comment `number`.
    Reply { number: u32 },

    /// A message beside the comments the next pass sends.
    Pass,
}

/// What a new comment is on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// The review as a whole.
    Review,

    /// The whole of `path`, as `version` holds it.
    File { path: String, version: u32 },

    /// The bytes `bytes` of `path`, as `version` holds it, which hold
    /// `excerpt`.
    Range {
        path: String,
        version: u32,
        bytes: [usize; 2],
        excerpt: String,
    },
}

impl Draft {
    /// A new comment on `on`.
    #[must_use]
    pub fn comment(on: Target) -> Self {
        Self {
            kind: Kind::Comment { on },
            body: String::new(),
            replacement: None,
        }
    }

    /// A new suggestion to change the range `on`, starting from the text it
    /// holds, Markdown syntax included.
    #[must_use]
    pub fn suggestion(on: Target) -> Self {
        let replacement = match &on {
            Target::Range { excerpt, .. } => Some(excerpt.clone()),
            Target::Review | Target::File { .. } => None,
        };

        Self {
            replacement,
            ..Self::comment(on)
        }
    }

    /// Changes to the draft `comment`, starting from what it says.
    #[must_use]
    pub fn edit(comment: &Comment) -> Option<Self> {
        let excerpt = comment
            .anchor
            .range
            .as_ref()
            .map(|_| comment.excerpt.clone().unwrap_or_default());

        Some(Self {
            kind: Kind::Edit {
                number: actions::number(comment)?,
                excerpt,
            },
            body: comment.body.clone(),
            replacement: comment.suggestion.clone(),
        })
    }

    /// The reviewer's reply on `comment`, starting from its draft reply.
    #[must_use]
    pub fn reply(comment: &Comment) -> Option<Self> {
        Some(Self {
            kind: Kind::Reply {
                number: actions::number(comment)?,
            },
            body: comment
                .draft_reply
                .as_ref()
                .map_or_default(|reply| reply.body.clone()),
            replacement: None,
        })
    }

    /// A message for the next pass.
    #[must_use]
    pub fn pass() -> Self {
        Self {
            kind: Kind::Pass,
            body: String::new(),
            replacement: None,
        }
    }
}

impl Kind {
    /// The text a suggestion replaces, when the draft is on a range and so can
    /// be one.
    #[must_use]
    pub fn excerpt(&self) -> Option<&str> {
        match self {
            Self::Comment {
                on: Target::Range { excerpt, .. },
            } => Some(excerpt),
            Self::Edit { excerpt, .. } => excerpt.as_deref(),
            Self::Comment { .. } | Self::Reply { .. } | Self::Pass => None,
        }
    }

    /// Whether a suggestion can be taken back: the API can't remove one from a
    /// draft comment, only replace it.
    #[must_use]
    pub fn can_unsuggest(&self) -> bool {
        matches!(self, Self::Comment { .. })
    }

    /// The composer's heading.
    #[must_use]
    pub fn title(&self) -> String {
        match self {
            Self::Comment { on: Target::Review } => "Comment on the review".to_owned(),
            Self::Comment {
                on: Target::File { path, version },
            } => format!("Comment on {path}, version {version}"),
            Self::Comment {
                on: Target::Range { path, version, .. },
            } => format!("Comment on a passage of {path}, version {version}"),
            Self::Edit { number, .. } => format!("Edit #{number}"),
            Self::Reply { number } => format!("Reply to #{number}"),
            Self::Pass => "Send to the assistant".to_owned(),
        }
    }

    /// The label of the button that saves the draft.
    #[must_use]
    pub fn save(&self) -> &'static str {
        match self {
            Self::Comment { .. } | Self::Reply { .. } => "Save draft",
            Self::Edit { .. } => "Save",
            Self::Pass => "Send",
        }
    }
}

/// Why `draft` can't be saved yet: a comment needs a body or a suggestion, and
/// a reply a body; a pass message may be empty.
#[must_use]
pub fn problem(draft: &Draft) -> Option<&'static str> {
    let empty = draft.body.trim().is_empty();
    match draft.kind {
        Kind::Comment { .. } | Kind::Edit { .. } if empty && draft.replacement.is_none() => {
            Some("Write a comment, or suggest a change.")
        }
        Kind::Reply { .. } if empty => Some("Write a reply."),
        _ => None,
    }
}

/// The request that saves `draft` to the review `id`.
#[must_use]
pub fn call(id: &str, draft: &Draft) -> Call {
    let body = draft.body.clone();
    match &draft.kind {
        Kind::Comment { on } => {
            let suggestion = match on {
                Target::Range { .. } => draft.replacement.clone(),
                Target::Review | Target::File { .. } => None,
            };
            Call::post(client::comments_url(id), &NewCommentRequest {
                anchor: anchor(on),
                body,
                suggestion,
            })
        }
        Kind::Edit { number, .. } => Call::patch(client::comment_url(id, *number), &EditRequest {
            anchor: None,
            body: Some(body),
            suggestion: draft.replacement.clone(),
        }),
        Kind::Reply { number } => {
            Call::post(client::replies_url(id, *number), &ReplyRequest { body })
        }
        Kind::Pass => Call::post(client::passes_url(id), &PassRequest {
            only: vec![],
            message: Some(body).filter(|body| !body.trim().is_empty()),
            conversation: None,
        }),
    }
}

/// What a request says a new comment on `on` is on.
fn anchor(on: &Target) -> Option<AnchorRequest> {
    match on {
        Target::Review => None,
        Target::File { path, version } => Some(AnchorRequest {
            version: Some(*version),
            path: Some(path.clone()),
            range: None,
            bytes: None,
        }),
        Target::Range {
            path,
            version,
            bytes,
            ..
        } => Some(AnchorRequest {
            version: Some(*version),
            path: Some(path.clone()),
            range: None,
            bytes: Some(*bytes),
        }),
    }
}

/// Where the composer of the review `id` is kept in local storage.
#[must_use]
pub fn key(id: &str) -> String {
    format!("jp-review:composer:{id}")
}

/// `draft` as local storage keeps it.
#[must_use]
pub fn encode(draft: &Draft) -> String {
    serde_json::to_string(draft).unwrap_or_default()
}

/// The draft local storage kept as `text`; `None` for text no draft reads from,
/// such as one an older UI wrote.
#[must_use]
pub fn decode(text: &str) -> Option<Draft> {
    serde_json::from_str(text).ok()
}

#[cfg(test)]
#[path = "composer_tests.rs"]
mod tests;
