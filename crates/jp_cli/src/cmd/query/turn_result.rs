//! What a finished turn hands back: its structured response, or its last
//! message.

use jp_conversation::{ConversationEvent, ConversationStream, EventId, event::ChatResponse};
use serde_json::{Map, Value};

use super::turn_start::last_request_id;

/// Why a turn has no structured response to hand back.
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

/// The structured response of the turn a run started.
///
/// `before` is the [`last_request_id`] from before the run.
/// When it is still the last request, the run appended nothing and there is no
/// answer of its own to give.
/// Otherwise only the conversation's last turn is searched, so an earlier
/// turn's data is never returned in its place.
///
/// A response that is not valid JSON is recorded as its raw text in a JSON
/// string.
/// When `schema` asks for something other than a string at its root, such a
/// string is refused: it is a response cut short, most often by the output
/// token limit, and not an answer.
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

/// The last message text the assistant wrote in the turn a run started.
///
/// `before` is the [`last_request_id`] from before the run, and bounds the
/// search the way it does for [`turn_structured_data`]: a run that appended no
/// request has no message of its own, and only the conversation's last turn is
/// searched.
/// A message holding only whitespace is skipped.
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
