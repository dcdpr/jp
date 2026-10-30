//! Turn-related event types.

use serde::{Deserialize, Serialize};

/// Marks the beginning of a new turn in the conversation.
///
/// A turn groups together the sequence of events from a user's chat request
/// through the assistant's final response, including any intermediate tool
/// calls.
/// It corresponds to a single `jp query` invocation.
///
/// The timestamp on the enclosing [`ConversationEvent`] records when the turn
/// started.
///
/// A [`ChatRequest`] event does NOT mark the beginning of a turn.
/// During a turn, a user might interrupt the assistant with a [`ChatRequest`],
/// but this happens within the context of a single turn.
///
/// [`ChatRequest`]: super::ChatRequest
/// [`ConversationEvent`]: super::ConversationEvent
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TurnStart {
    /// A single-line summary of what the turn asked and accomplished, used to
    /// identify the turn when navigating or editing the conversation.
    ///
    /// `None` while the turn has no summary.
    /// Changing it leaves the conversation's provider requests untouched:
    /// `TurnStart` events are not provider-visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}
