//! Turn-related event types.

use serde::{Deserialize, Serialize};

/// Maximum length of a turn summary, in Unicode scalar values.
///
/// The prompt asks for at most 60 characters; longer model outputs and
/// mechanical fallbacks are truncated to 72 characters with a trailing `…`.
pub const MAX_SUMMARY_CHARS: usize = 72;

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
    /// Single-line summary of what the turn asked and accomplished.
    ///
    /// Used as display metadata when navigating or editing the conversation.
    /// `TurnStart` is not provider-visible, so setting or clearing this field
    /// does not affect provider requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Normalize `text` into a single-line turn summary of at most
/// [`MAX_SUMMARY_CHARS`] characters.
///
/// Consecutive whitespace characters (including newlines) are collapsed to a
/// single space.
/// Strings longer than [`MAX_SUMMARY_CHARS`] are truncated and end with `…`.
/// Returns `None` if `text` contains only whitespace.
#[must_use]
pub fn format_turn_summary(text: &str) -> Option<String> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }

    if line.chars().count() <= MAX_SUMMARY_CHARS {
        return Some(line);
    }

    let truncated: String = line.chars().take(MAX_SUMMARY_CHARS - 1).collect();
    Some(format!("{}…", truncated.trim_end()))
}
