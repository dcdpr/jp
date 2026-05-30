use eventsource_stream::Event as MessageEvent;
use futures::StreamExt as _;
use reqwest_eventsource::Error as SseError;

use super::*;
use crate::provider::llamacpp::StreamChunk;

fn sse_message(data: &str) -> SseEvent {
    SseEvent::Message(MessageEvent {
        data: data.to_owned(),
        ..MessageEvent::default()
    })
}

fn flush_indices(events: &[std::result::Result<Event, StreamError>]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|e| match e {
            Ok(Event::Flush { index, .. }) => Some(*index),
            _ => None,
        })
        .collect()
}

#[test_log::test(tokio::test)]
async fn surfaces_stream_error_before_completion() {
    // A transport error before `[DONE]` (a dropped or stalled connection) must
    // surface as a `StreamError` so the retry layer can act on it, rather than
    // being silently swallowed.
    let content = sse_message(
        r#"{"choices":[{"delta":{"content":"partial"},"index":0,"finish_reason":null}]}"#,
    );
    let events = stream::iter(vec![Ok(content), Err(SseError::StreamEnded)]);

    let out: Vec<_> = assemble_event_stream(events, false).collect().await;

    assert!(
        out.iter().any(std::result::Result::is_err),
        "pre-completion stream error must surface, got {out:?}",
    );
}

#[test_log::test(tokio::test)]
async fn swallows_stream_error_after_completion() {
    // The connection close that follows `[DONE]` is the benign EOF; once the
    // stream has emitted `Finished` it must not be surfaced as an error.
    let content =
        sse_message(r#"{"choices":[{"delta":{"content":"hi"},"index":0,"finish_reason":"stop"}]}"#);
    let events = stream::iter(vec![
        Ok(content),
        Ok(sse_message("[DONE]")),
        Err(SseError::StreamEnded),
    ]);

    let out: Vec<_> = assemble_event_stream(events, false).collect().await;

    assert!(
        out.iter().all(std::result::Result::is_ok),
        "post-completion close must not surface an error, got {out:?}",
    );
    assert!(
        matches!(out.last(), Some(Ok(Event::Finished(_)))),
        "stream must end with Finished, got {:?}",
        out.last(),
    );
}

#[test]
fn parse_cerebras_content_chunk() {
    let json = r#"{
        "choices": [{
            "delta": { "content": "Hello!" },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello!"));
    assert!(chunk.choices[0].finish_reason.is_none());
}

#[test]
fn parse_cerebras_reasoning_field() {
    // Cerebras parsed format returns reasoning in the `reasoning` field, which
    // is deserialized into `reasoning_content` via serde alias.
    let json = r#"{
        "choices": [{
            "delta": {
                "reasoning": "The user just says hello.",
                "content": null
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert_eq!(
        delta.reasoning_content.as_deref(),
        Some("The user just says hello.")
    );
    assert!(delta.content.is_none());
}

#[test]
fn parse_cerebras_reasoning_content_field() {
    // The `reasoning_content` field name also works (DeepSeek-compatible).
    let json = r#"{
        "choices": [{
            "delta": {
                "reasoning_content": "step by step",
                "content": null
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert_eq!(delta.reasoning_content.as_deref(), Some("step by step"));
}

#[test]
fn parse_cerebras_finish_reason_stop() {
    let json = r#"{
        "choices": [{
            "delta": {},
            "index": 0,
            "finish_reason": "stop"
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
}

#[test]
fn parse_cerebras_tool_call() {
    let json = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_xyz",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"foo.rs\"}"
                    }
                }]
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
    assert_eq!(tc.id.as_deref(), Some("call_xyz"));
    assert_eq!(
        tc.function.as_ref().unwrap().name.as_deref(),
        Some("read_file")
    );
}

#[test]
fn convert_tool_choice_values() {
    assert_eq!(convert_tool_choice(&ToolChoice::Auto), json!("auto"));
    assert_eq!(convert_tool_choice(&ToolChoice::None), json!("none"));
    assert_eq!(
        convert_tool_choice(&ToolChoice::Required),
        json!("required")
    );
    assert_eq!(
        convert_tool_choice(&ToolChoice::Function("my_fn".into())),
        json!({"type": "function", "function": {"name": "my_fn"}})
    );
}

#[test]
fn map_model_known() {
    let details = map_model("llama3.1-8b").unwrap();
    assert_eq!(details.display_name.as_deref(), Some("Llama 3.1 8B"));
    assert_eq!(details.context_window, Some(32_768));
    assert_eq!(details.max_output_tokens, Some(8_192));
    assert!(details.reasoning.unwrap().is_unsupported());
}

#[test]
fn map_model_gpt_oss_has_reasoning() {
    let details = map_model("gpt-oss-120b").unwrap();
    assert!(details.reasoning.unwrap().is_leveled());
}

#[test]
fn map_model_unknown_returns_empty() {
    let details = map_model("some-future-model").unwrap();
    assert!(details.display_name.is_none());
    assert!(details.context_window.is_none());
}

#[test]
fn transform_schema_moves_array_constraints_to_description() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "tags": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1,
                "maxItems": 5
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let tags = &result["properties"]["tags"];

    // Unsupported fields removed from schema.
    assert!(tags.get("minItems").is_none());
    assert!(tags.get("maxItems").is_none());

    // But preserved as a description hint.
    let desc = tags["description"].as_str().unwrap();
    assert!(desc.contains("minItems"), "desc = {desc}");
    assert!(desc.contains("maxItems"), "desc = {desc}");

    // Supported fields still present.
    assert_eq!(tags["type"], "array");
    assert_eq!(tags["items"]["type"], "string");
}

#[test]
fn transform_schema_moves_string_constraints_to_description() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "email": {
                "type": "string",
                "description": "An email address",
                "format": "email",
                "pattern": "^.+@.+$"
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let email = &result["properties"]["email"];

    assert!(email.get("format").is_none());
    assert!(email.get("pattern").is_none());

    // Hints appended to existing description.
    let desc = email["description"].as_str().unwrap();
    assert!(desc.starts_with("An email address"), "desc = {desc}");
    assert!(desc.contains("format"), "desc = {desc}");
    assert!(desc.contains("pattern"), "desc = {desc}");
}

#[test]
fn transform_schema_forces_strict_objects() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "nested": {
                "type": "object",
                "properties": {
                    "value": { "type": "integer" }
                }
            }
        },
        "required": ["name"]
    }))
    .unwrap();

    let result = transform_schema(schema);

    // Root object: additionalProperties false, all props required.
    assert_eq!(result["additionalProperties"], false);
    let required = result["required"].as_array().unwrap();
    assert!(required.contains(&json!("name")));
    assert!(required.contains(&json!("nested")));

    // Nested object: same treatment.
    let nested = &result["properties"]["nested"];
    assert_eq!(nested["additionalProperties"], false);
    let nested_req = nested["required"].as_array().unwrap();
    assert!(nested_req.contains(&json!("value")));
}

#[test]
fn transform_schema_preserves_number_constraints() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "age": {
                "type": "integer",
                "minimum": 0,
                "maximum": 150
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let age = &result["properties"]["age"];
    assert_eq!(age["minimum"], 0);
    assert_eq!(age["maximum"], 150);
    assert!(age.get("description").is_none());
}

/// `finish_reason: "length"` followed by `[DONE]` must not flush any pending
/// tool-call buffers.
/// When the model hits the token limit mid-tool-call, the arguments are
/// structurally incomplete; the safety-net drain on `[DONE]` would otherwise
/// commit them with truncated JSON (degraded to `{}`), which could re-dispatch
/// a partial call.
#[test]
fn length_finish_reason_drops_pending_tool_calls() {
    let mut state = StreamState {
        tool_call_indices: Vec::new(),
        reasoning_flushed: false,
        message_flushed: false,
        finished: false,
        finish_reason: None,
        is_structured: false,
    };

    // Tool call delta with partial arguments.
    let tool_chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_xyz",
                    "function": { "name": "run_me", "arguments": "{\"path\":" }
                }]
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;
    handle_sse_event_sync(Ok(sse_message(tool_chunk)), &mut state).unwrap();
    assert_eq!(state.tool_call_indices, vec![2]);

    // Terminal `"length"` chunk: should clear the pending tool-call index so
    // the `[DONE]` safety net cannot commit the truncated buffer.
    let finish_chunk = r#"{
        "choices": [{
            "delta": {},
            "index": 0,
            "finish_reason": "length"
        }]
    }"#;
    let finish_events = handle_sse_event_sync(Ok(sse_message(finish_chunk)), &mut state).unwrap();
    // Reasoning was already flushed when the tool-call chunk arrived, so only
    // the message index flushes here. The tool-call index must NOT be in this
    // list — that's the bug guard.
    assert_eq!(
        flush_indices(&finish_events),
        vec![1],
        "only message index should flush on length, got {finish_events:?}"
    );
    assert!(
        state.tool_call_indices.is_empty(),
        "length must drop pending tool-call indices, got {:?}",
        state.tool_call_indices,
    );
    assert_eq!(state.finish_reason, Some(FinishReason::MaxTokens));

    // `[DONE]` safety net: must NOT flush the tool-call index, and must
    // finish with MaxTokens.
    let done_events = handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap();
    assert!(
        flush_indices(&done_events).is_empty(),
        "[DONE] after length must not flush any indices, got {done_events:?}"
    );
    let last = done_events.last().unwrap().as_ref().unwrap();
    assert!(
        matches!(last, Event::Finished(FinishReason::MaxTokens)),
        "expected Finished(MaxTokens), got {last:?}"
    );
}
