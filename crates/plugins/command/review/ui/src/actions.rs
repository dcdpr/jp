//! What a reviewer can do with a comment, and which comments the next pass
//! sends.

use jp_review_api::{Comment, Role, Status};

/// Something a comment's card offers to do with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Change a draft comment.
    Edit,

    /// Delete a draft comment.
    Withdraw,

    /// Write a reply to a sent comment, which the next pass sends.
    Reply,

    /// Change the draft reply on a sent comment.
    EditReply,

    /// Delete the draft reply on a sent comment.
    WithdrawReply,

    /// Close a sent comment.
    Resolve,

    /// Open a resolved comment again.
    Reopen,
}

impl Action {
    /// The action's button label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Withdraw => "Withdraw",
            Self::Reply => "Reply",
            Self::EditReply => "Edit reply",
            Self::WithdrawReply => "Withdraw reply",
            Self::Resolve => "Resolve",
            Self::Reopen => "Reopen",
        }
    }
}

/// What a card offers to do with `comment`: edit or withdraw a draft; reply, or
/// edit or withdraw the draft reply, on a sent one, and resolve it; and open a
/// resolved one again.
#[must_use]
pub fn actions(comment: &Comment) -> Vec<Action> {
    match (comment.status, comment.draft_reply.is_some()) {
        (Status::Draft, _) => vec![Action::Edit, Action::Withdraw],
        (Status::Open, false) => vec![Action::Reply, Action::Resolve],
        (Status::Open, true) => vec![Action::EditReply, Action::WithdrawReply, Action::Resolve],
        (Status::Resolved, _) => vec![Action::Reopen],
    }
}

/// The number of `comment` in its review: `3` of `R-1h4k2m9#3`.
#[must_use]
pub fn number(comment: &Comment) -> Option<u32> {
    comment.id.rsplit_once('#')?.1.parse().ok()
}

/// Whether the next pass sends `comment`: it is a draft, holds a draft reply,
/// or the reviewer wrote in it last.
#[must_use]
pub fn awaits_assistant(comment: &Comment) -> bool {
    let last = comment
        .replies
        .last()
        .map_or(comment.role, |reply| reply.role);

    match comment.status {
        Status::Resolved => false,
        Status::Draft => true,
        Status::Open => comment.draft_reply.is_some() || last == Role::Reviewer,
    }
}

/// How many of `comments` the next pass sends.
#[must_use]
pub fn pending(comments: &[Comment]) -> usize {
    comments
        .iter()
        .filter(|comment| awaits_assistant(comment))
        .count()
}

#[cfg(test)]
#[path = "actions_tests.rs"]
mod tests;
