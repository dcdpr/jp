//! Extract the result of a completed turn: its structured response or its last
//! assistant message.

use jp_conversation::{ConversationEvent, ConversationStream, EventId, event::ChatResponse};
use serde_json::{Map, Value};

use super::turn_start::last_request_id;

/// Reasons a turn has no structured response to return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum StructuredDataError {
    #[error("the turn was stopped before it started")]
    NotStarted,

    #[error("no structured data in the assistant's response")]
    Missing,

    #[error(
        "the assistant's structured response is not valid JSON; it may have been cut off at the \
         output token limit"
    )]
    NotJson,
}

/// Extract the structured response from the turn started by this run.
///
/// `before` is the [`last_request_id`] captured before the turn ran.
/// If the stream's last request ID still matches `before`, the turn was stopped
/// before appending its request and returns
/// [`StructuredDataError::NotStarted`].
/// Otherwise, only the conversation's last turn is searched so data from an
/// earlier turn is never returned.
///
/// When the provider's response is not valid JSON, it is stored as a raw JSON
/// string.
/// If `schema` specifies a root `type` other than `"string"`, a string value is
/// rejected with [`StructuredDataError::NotJson`] (typically caused by hitting
/// the output token limit).
pub(crate) fn turn_structured_data(
    events: &ConversationStream,
    before: Option<&EventId>,
    schema: Option<&Map<String, Value>>,
) -> std::result::Result<Value, StructuredDataError> {
    if last_request_id(events).as_ref() == before {
        return Err(StructuredDataError::NotStarted);
    }

    let data = events
        .last_turn_events()
        .rev()
        .find_map(|event| {
            event
                .as_chat_response()
                .and_then(ChatResponse::as_structured_data)
                .cloned()
        })
        .ok_or(StructuredDataError::Missing)?;

    // Only a root `type` naming something other than `string` rules a string
    // out. A schema that says less than that takes the response at its word.
    let admits_string = schema
        .and_then(|schema| schema.get("type"))
        .and_then(Value::as_str)
        .is_none_or(|kind| kind == "string");

    if data.is_string() && !admits_string {
        return Err(StructuredDataError::NotJson);
    }

    Ok(data)
}

/// Extract the last non-empty assistant message from the turn started by this
/// run.
///
/// `before` is the [`last_request_id`] captured before the turn ran.
/// Returns `None` if no new request was appended or if the last turn contains
/// no non-whitespace assistant message.
pub(crate) fn turn_answer(events: &ConversationStream, before: Option<&EventId>) -> Option<String> {
    if last_request_id(events).as_ref() == before {
        return None;
    }

    events
        .last_turn_events()
        .rev()
        .filter_map(ConversationEvent::as_chat_response)
        .filter_map(ChatResponse::as_message)
        .find(|message| !message.trim().is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
#[path = "turn_result_tests.rs"]
mod tests;
