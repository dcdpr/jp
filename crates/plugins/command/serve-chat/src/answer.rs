//! Extract the assistant's final response from a conversation's event stream.
//!
//! A turn often streams an initial message (such as "Let me look"), runs tool
//! calls, and then streams the final answer.
//! Only the text after the last tool call or chat request is sent back to the
//! chat.
//! Reasoning chunks are ignored.

use serde_json::Value;

/// Extract the assistant's final message from the last turn in `events`.
///
/// `events` is the conversation event stream returned by `read_events`.
/// If the turn ended on a tool call with no trailing message, falls back to the
/// last non-empty assistant message earlier in the turn.
/// Returns `None` if the turn produced no assistant message text.
pub(crate) fn last_answer(events: &[Value]) -> Option<String> {
    let start = events
        .iter()
        .rposition(|event| kind(event) == Some("turn_start"))
        .map_or(0, |index| index + 1);

    let mut answer: Option<String> = None;
    let mut current = String::new();
    for event in &events[start..] {
        match kind(event) {
            Some("chat_response") => {
                if let Some(text) = event.get("message").and_then(Value::as_str) {
                    current.push_str(text);
                }
            }
            Some("tool_call_request" | "chat_request") => {
                if !current.trim().is_empty() {
                    answer = Some(current.trim().to_owned());
                }
                current.clear();
            }
            _ => {}
        }
    }

    if current.trim().is_empty() {
        answer
    } else {
        Some(current.trim().to_owned())
    }
}

fn kind(event: &Value) -> Option<&str> {
    event.get("type").and_then(Value::as_str)
}

#[cfg(test)]
#[path = "answer_tests.rs"]
mod tests;
