//! Semantic parity between the platform API and subscription wire dialects.
//!
//! The two endpoints cannot receive byte-identical requests: the subscription
//! endpoint requires system content in `instructions`, rejects explicit cache
//! breakpoints, and rejects several platform parameters.
//! This projection strips those transport differences and compares the request
//! facts that both routes promise to preserve.

use serde_json::{Value, json};

use super::{create_request, prepare_subscription_request};
use crate::{model::ModelDetails, query::ChatQuery};

/// Assert that rewriting a request for the subscription host preserves its
/// meaning.
///
/// A property of [`prepare_subscription_request`] alone: one request is built,
/// a copy is rewritten, and the two projections are compared.
/// The comparison across two recordings is [`project`]'s other caller, in the
/// test harness.
pub(crate) fn assert_rewrite_preserves_meaning(model: &ModelDetails, query: ChatQuery) {
    let (api, _, _) = create_request(model, query).expect("a valid OpenAI test request");
    let mut subscription = api.clone();
    prepare_subscription_request(&mut subscription);

    let api = project(&api);
    let subscription = project(&subscription);

    assert_eq!(
        api, subscription,
        "OpenAI API and subscription requests differ semantically"
    );
}

/// The request facts both billing routes promise to preserve.
///
/// Fields absent from this projection are measured endpoint differences, not
/// ignored semantics:
///
/// - `max_output_tokens`, sampling controls, truncation, metadata, user and
///   previous-response state are unsupported by the subscription endpoint.
/// - explicit cache options and content breakpoints are unsupported there;
///   `prompt_cache_key` is preserved and compared.
/// - `stream` is a transport choice made by the client method.
fn project(request: &openai_responses::types::Request) -> Value {
    let request = serde_json::to_value(request).expect("a serializable OpenAI request");
    project_body(&request)
}

/// The same projection, over a request body read back from a recording.
pub(crate) fn project_body(request: &Value) -> Value {
    let request = request.clone();
    let system = system_prompt(&request);
    let input = conversation_input(&request);

    let mut projected = json!({
        "model": request.get("model"),
        "system": system,
        "input": input,
        "include": request.get("include"),
        "parallel_tool_calls": request.get("parallel_tool_calls"),
        "prompt_cache_key": request.get("prompt_cache_key"),
        "reasoning": request.get("reasoning"),
        "service_tier": request.get("service_tier"),
        "store": request.get("store"),
        "text": request.get("text"),
        "tool_choice": request.get("tool_choice"),
        "tools": request.get("tools"),
    });

    // Both are minted by the host: `id` on a replayed message or reasoning
    // item, `call_id` pairing a function call with its output.
    super::super::number_ids(&mut projected, &["id", "call_id"]);

    projected
}

/// Erase what the model contributed to one input item.
///
/// A key is only model-authored where the item type says so: `text` belongs to
/// the model inside an assistant message and to JP inside a system one, and
/// `arguments` is a tool call's while `input` at the top of the body is the
/// whole conversation.
fn erase_model_output(item: &mut Value) {
    let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
    let role = item.get("role").and_then(Value::as_str).unwrap_or_default();

    match kind {
        "message" if role == "assistant" => {
            if let Some(content) = item.get_mut("content") {
                replace_text(content);
            }
        }

        // The summary is the model's account of its own reasoning; the
        // encrypted blob is that reasoning in a form only the host reads.
        "reasoning" => {
            if let Some(summary) = item.get_mut("summary") {
                replace_text(summary);
            }
            if let Some(encrypted) = item.get_mut("encrypted_content")
                && encrypted.is_string()
            {
                *encrypted = Value::from("<opaque>");
            }
        }

        "function_call" => {
            if let Some(arguments) = item.get_mut("arguments") {
                *arguments = Value::from("<model arguments>");
            }
        }

        _ => {}
    }
}

/// Stand a placeholder in for every `text` under one content value.
fn replace_text(value: &mut Value) {
    match value {
        Value::Array(values) => values.iter_mut().for_each(replace_text),
        Value::Object(object) => {
            for (key, value) in object.iter_mut() {
                if key == "text" && value.is_string() {
                    *value = Value::from("<model prose>");
                } else {
                    replace_text(value);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Read system content from either dialect.
fn system_prompt(request: &Value) -> String {
    if let Some(instructions) = request.get("instructions").and_then(Value::as_str) {
        return instructions.to_owned();
    }

    request
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("role").and_then(Value::as_str) == Some("system"))
        .flat_map(message_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Read every text block from one input message.
fn message_text(message: &Value) -> Vec<String> {
    let Some(content) = message.get("content") else {
        return vec![];
    };

    if let Some(text) = content.as_str() {
        return vec![text.to_owned()];
    }

    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// Conversation input, with system messages, cache-marker syntax, and
/// everything the model authored removed.
fn conversation_input(request: &Value) -> Value {
    let mut input = request
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("role").and_then(Value::as_str) != Some("system"))
        .cloned()
        .collect::<Vec<_>>();

    for item in &mut input {
        remove_cache_markers(item);
        erase_model_output(item);
    }

    Value::Array(input)
}

/// Remove explicit cache-marker syntax wherever a content block carries it.
fn remove_cache_markers(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                remove_cache_markers(value);
            }
        }
        Value::Object(object) => {
            object.remove("prompt_cache_breakpoint");
            for value in object.values_mut() {
                remove_cache_markers(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};
    use jp_conversation::{
        ConversationStream,
        event::{ChatRequest, ConversationEvent, TurnStart},
        thread::ThreadBuilder,
    };

    use super::*;
    use crate::provider::openai::{SHARED_TEST_MODEL, catalog_model_details};

    #[test]
    fn plain_request_has_semantic_parity() {
        let timestamp = Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap();
        let mut events = ConversationStream::new_test().with_created_at(timestamp);
        events.extend([
            ConversationEvent::new(TurnStart, timestamp),
            ConversationEvent::new(ChatRequest::from("hello"), timestamp),
        ]);
        let query = ChatQuery::from(
            ThreadBuilder::new()
                .with_system_prompt("You are JP.")
                .with_events(events)
                .build()
                .unwrap(),
        );

        assert_rewrite_preserves_meaning(&catalog_model_details(SHARED_TEST_MODEL), query);
    }
}
