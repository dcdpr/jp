//! Turn-related event types.

use serde::{Deserialize, Serialize};

/// Marks the beginning of a new turn in the conversation.
///
/// A turn groups together the sequence of events from a user's chat request
/// through the assistant's final response, including any intermediate tool
/// calls. It corresponds to a single `jp query` invocation.
///
/// The timestamp on the enclosing [`ConversationEvent`] records when the
/// turn started.
///
/// A [`ChatRequest`] event does NOT mark the beginning of a turn. During a
/// turn, a user might interrupt the assistant with a [`ChatRequest`], but this
/// happens within the context of a single turn.
///
/// [`ConversationEvent`]: super::ConversationEvent
/// [`ChatRequest`]: super::ChatRequest
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TurnStart;
