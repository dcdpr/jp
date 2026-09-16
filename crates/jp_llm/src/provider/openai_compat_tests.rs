use eventsource_stream::Event as MessageEvent;
use jp_conversation::{ConversationEvent, event::ToolCallRequest};
use reqwest_eventsource::Error as SseError;
use serde_json::json;

use super::*;
use crate::event::EventPart;

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

#[test_log::test(tokio::test)]
async fn surfaces_stream_error_before_completion() {
    // A transport error before `[DONE]` (a dropped or stalled connection) must
    // surface as a `StreamError` so the retry layer can act on it, rather than
    // being silently swallowed.
    let content = sse_message(
        r#"{"choices":[{"delta":{"content":"partial"},"index":0,"finish_reason":null}]}"#,
    );
    let events = stream::iter(vec![Ok(content), Err(SseError::StreamEnded)]);

    let out: Vec<_> = assemble_event_stream(events, "test", false).collect().await;

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

    let out: Vec<_> = assemble_event_stream(events, "test", false).collect().await;

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

/// `finish_reason: "length"` followed by `[DONE]` must not flush any pending
/// tool-call buffers.
/// When the model hits the token limit mid-tool-call, the arguments are
/// structurally incomplete; the safety-net drain on `[DONE]` would otherwise
/// commit them with truncated JSON (degraded to `{}`), which could re-dispatch
/// a partial call.
#[test]
fn length_finish_reason_drops_pending_tool_calls() {
    let mut state = StreamState::new("test", false);

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
    // list.
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

/// A tool-call frame must release the extractor's held-back tail before
/// emitting any tool-call parts.
///
/// The `ReasoningExtractor` withholds the last bytes of content (one less than
/// the `<think>\n` opener) in case a tag is split across frames.
/// Downstream drains the in-progress markdown paragraph at the tool-call
/// boundary, so if the tail were released after `ToolCallPart::Start`, it would
/// land in a fresh paragraph and render as a mid-word blank-line split (e.g.
/// `…directo` then a blank line then `ries.`).
#[test]
fn tool_call_frame_releases_extractor_tail_before_tool_call_parts() {
    let mut state = StreamState::new("test", false);

    // A full paragraph in one frame, ending in a word long enough that the
    // hold-back window splits it.
    let content =
        "Let me first check what tools are available to me for reading files and directories.\n\n";
    let content_chunk = json!({
        "choices": [{
            "delta": { "content": content },
            "index": 0,
            "finish_reason": null
        }]
    });
    let content_events =
        handle_sse_event_sync(Ok(sse_message(&content_chunk.to_string())), &mut state).unwrap();

    // The content frame withholds the tail while tag detection stays armed.
    let content_emitted: String = content_events
        .iter()
        .filter_map(|e| match e.as_ref().ok() {
            Some(Event::Part {
                part: EventPart::Message(text),
                ..
            }) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !content_emitted.ends_with("directories.\n\n"),
        "the tail should still be held back after the content frame: {content_emitted:?}"
    );

    // The tool-call frame releases the tail...
    let tool_chunk = json!({
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "function": { "name": "describe_tools", "arguments": "{}" }
                }]
            },
            "index": 0,
            "finish_reason": "tool_calls"
        }]
    });
    let tool_events =
        handle_sse_event_sync(Ok(sse_message(&tool_chunk.to_string())), &mut state).unwrap();

    let tail: String = tool_events
        .iter()
        .filter_map(|e| match e.as_ref().ok() {
            Some(Event::Part {
                part: EventPart::Message(text),
                ..
            }) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        format!("{content_emitted}{tail}"),
        content,
        "content must be preserved across the tool-call boundary"
    );

    // ...and it must precede every tool-call part in the emitted order, so the
    // downstream paragraph drain at the tool-call boundary sees the complete
    // paragraph.
    let first_tool_call = tool_events
        .iter()
        .position(|e| {
            matches!(
                e.as_ref().ok(),
                Some(Event::Part {
                    part: EventPart::ToolCall(_),
                    ..
                })
            )
        })
        .unwrap();
    let last_message = tool_events
        .iter()
        .rposition(|e| {
            matches!(
                e.as_ref().ok(),
                Some(Event::Part {
                    part: EventPart::Message(_),
                    ..
                })
            )
        })
        .unwrap();
    assert!(
        last_message < first_tool_call,
        "the extractor tail must be emitted before the tool-call parts, got {tool_events:?}"
    );
}

#[test]
fn convert_events_merges_consecutive_tool_calls() {
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

/// A chunk carrying content parses and is handed back.
#[test]
fn parse_chunk_accepts_an_ordinary_chunk() {
    let data = r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#;

    let chunk = parse_chunk(data, "test").expect("chunk with choices is kept");

    assert_eq!(chunk.choices.len(), 1);
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
}

#[test]
fn parse_chunk_drops_a_malformed_payload() {
    assert!(parse_chunk("{not json", "test").is_none());
}

/// A chunk with no choices yields no events, so there is nothing to hand back.
///
/// This is the benign shape: a server that reports usage in its own chunk sends
/// one, and it is not worth a warning.
#[test]
fn parse_chunk_drops_a_chunk_without_choices() {
    let data = r#"{"choices":[],"usage":{"total_tokens":7}}"#;

    assert!(parse_chunk(data, "test").is_none());
}

/// llama.cpp opens every stream with a role-only delta, and repeats it on each
/// progress update.
/// It carries a choice, but nothing a handler can emit.
#[test]
fn parse_chunk_drops_a_role_only_chunk() {
    let data = r#"{"choices":[{"finish_reason":null,"index":0,"delta":{"role":"assistant","content":null}}]}"#;

    assert!(parse_chunk(data, "test").is_none());
}

/// An error reported inside the stream is the shape worth noticing.
///
/// The chunk types ignore unknown fields, so before `error` was captured this
/// payload deserialized into a chunk with an empty `choices` and was
/// indistinguishable from the benign case above.
#[test]
fn parse_chunk_captures_an_in_stream_error() {
    let data = r#"{"error":{"message":"upstream exploded","type":"server_error"}}"#;

    let chunk: StreamChunk = serde_json::from_str(data).expect("chunk parses");

    assert_eq!(
        chunk.error,
        Some(json!({"message":"upstream exploded","type":"server_error"}))
    );
    assert!(parse_chunk(data, "test").is_none());
}

/// Capturing `error` must not disturb the ordinary path.
#[test]
fn an_ordinary_chunk_carries_no_error() {
    let data = r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#;

    let chunk: StreamChunk = serde_json::from_str(data).expect("chunk parses");

    assert_eq!(chunk.error, None);
}
