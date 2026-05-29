use eventsource_stream::Event as MessageEvent;
use jp_conversation::ConversationEvent;

use super::*;

fn sse_message(data: &str) -> SseEvent {
    SseEvent::Message(MessageEvent {
        data: data.to_owned(),
        ..MessageEvent::default()
    })
}

fn flush_indices(events: &[Result<Event, StreamError>]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|e| match e {
            Ok(Event::Flush { index, .. }) => Some(*index),
            _ => None,
        })
        .collect()
}

#[test]
fn parse_deepseek_format_reasoning_in_dedicated_field() {
    // The default `--reasoning-format deepseek`: reasoning arrives in
    // `reasoning_content`, regular content in `content`.
    let json = r#"{
            "choices": [{
                "delta": {
                    "reasoning_content": "Let me think step by step...",
                    "content": null
                },
                "index": 0,
                "finish_reason": null
            }]
        }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.choices.len(), 1);

    let delta = &chunk.choices[0].delta;
    assert_eq!(
        delta.reasoning_content.as_deref(),
        Some("Let me think step by step...")
    );
    assert!(delta.content.is_none());
}

#[test]
fn parse_deepseek_format_content_after_reasoning() {
    let json = r#"{
            "choices": [{
                "delta": {
                    "reasoning_content": null,
                    "content": "The answer is 42."
                },
                "index": 0,
                "finish_reason": null
            }]
        }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert!(delta.reasoning_content.is_none());
    assert_eq!(delta.content.as_deref(), Some("The answer is 42."));
}

#[test]
fn parse_none_format_think_tags_in_content() {
    // `--reasoning-format none`: everything in `content`, with <think> tags.
    // No `reasoning_content` field at all.
    let json = r#"{
            "choices": [{
                "delta": {
                    "content": "<think>\nLet me reason...\n</think>\nThe answer."
                },
                "index": 0,
                "finish_reason": null
            }]
        }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert!(delta.reasoning_content.is_none());
    assert!(delta.content.as_ref().unwrap().contains("<think>"));
}

#[test]
fn parse_finish_reason() {
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
fn parse_tool_call_delta() {
    let json = r#"{
            "choices": [{
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_abc123",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"city\":"
                        }
                    }]
                },
                "index": 0,
                "finish_reason": null
            }]
        }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let tool_calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id.as_deref(), Some("call_abc123"));
    let func = tool_calls[0].function.as_ref().unwrap();
    assert_eq!(func.name.as_deref(), Some("get_weather"));
    assert_eq!(func.arguments.as_deref(), Some("{\"city\":"));
}

#[test]
fn parse_empty_choices() {
    // Some servers send empty choices arrays (e.g. usage-only chunks).
    let json = r#"{"choices": []}"#;
    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert!(chunk.choices.is_empty());
}

#[test]
fn parse_missing_optional_fields() {
    // Minimal delta with only content.
    let json = r#"{"choices": [{"delta": {"content": "hi"}}]}"#;
    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert_eq!(delta.content.as_deref(), Some("hi"));
    assert!(delta.reasoning_content.is_none());
    assert!(delta.tool_calls.is_none());
    assert!(chunk.choices[0].finish_reason.is_none());
}

#[test]
fn convert_events_merges_consecutive_tool_calls() {
    use jp_conversation::event::ToolCallRequest;

    let mut events = ConversationStream::new_test();
    events.extend([
        ConversationEvent::now(ToolCallRequest {
            id: "call_1".into(),
            name: "tool_a".into(),
            arguments: serde_json::Map::new(),
        }),
        ConversationEvent::now(ToolCallRequest {
            id: "call_2".into(),
            name: "tool_b".into(),
            arguments: serde_json::Map::new(),
        }),
    ]);

    let messages = convert_events(events);

    // Should be merged into a single assistant message with 2 tool_calls.
    assert_eq!(messages.len(), 1);
    let tool_calls = messages[0]["tool_calls"].as_array().unwrap();
    assert_eq!(tool_calls.len(), 2);
    assert_eq!(tool_calls[0]["function"]["name"], "tool_a");
    assert_eq!(tool_calls[1]["function"]["name"], "tool_b");
}

#[test]
fn convert_events_sends_reasoning_content_field() {
    let mut events = ConversationStream::new_test();
    events.extend(std::iter::once(ConversationEvent::now(
        ChatResponse::reasoning("step 1: think hard"),
    )));

    let messages = convert_events(events);

    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["reasoning_content"].as_str().unwrap(),
        "step 1: think hard"
    );
}

#[test]
fn convert_events_merges_reasoning_and_message() {
    let mut events = ConversationStream::new_test();
    events.extend([
        ConversationEvent::now(ChatResponse::reasoning("let me think...")),
        ConversationEvent::now(ChatResponse::message("the answer is 42")),
    ]);

    let messages = convert_events(events);

    // Reasoning + message should be merged into a single assistant message.
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0]["reasoning_content"].as_str().unwrap(),
        "let me think..."
    );
    assert_eq!(messages[0]["content"].as_str().unwrap(), "the answer is 42");
}

#[test]
fn convert_tool_choice_values() {
    assert_eq!(convert_tool_choice(&ToolChoice::Auto), "auto");
    assert_eq!(convert_tool_choice(&ToolChoice::None), "none");
    assert_eq!(convert_tool_choice(&ToolChoice::Required), "required");
    assert_eq!(
        convert_tool_choice(&ToolChoice::Function("my_fn".into())),
        "required"
    );
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
        extractor: ReasoningExtractor::default(),
        tool_call_indices: Vec::new(),
        reasoning_flushed: false,
        message_flushed: false,
        finish_reason: None,
        is_structured: false,
    };

    // Tool call delta with partial arguments.
    let tool_chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_abc",
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
