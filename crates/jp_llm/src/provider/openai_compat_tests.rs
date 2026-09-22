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

fn message_text(events: &[Result<Event, StreamError>]) -> String {
    events
        .iter()
        .filter_map(|e| match e.as_ref().ok() {
            Some(Event::Part {
                part: EventPart::Message(text),
                ..
            }) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn reasoning_text(events: &[Result<Event, StreamError>]) -> String {
    events
        .iter()
        .filter_map(|e| match e.as_ref().ok() {
            Some(Event::Part {
                part: EventPart::Reasoning(text),
                ..
            }) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn strips_the_template_separator_between_reasoning_and_content() {
    let mut state = StreamState::new("test", false);

    let reasoning = json!({
        "choices": [{
            "delta": { "reasoning": "Deciding what to say.\n" },
            "index": 0,
            "finish_reason": null
        }]
    });
    handle_sse_event_sync(Ok(sse_message(&reasoning.to_string())), &mut state).unwrap();

    let content = json!({
        "choices": [{
            "delta": { "content": "\n\nTest received." },
            "index": 0,
            "finish_reason": null
        }]
    });
    let mut events =
        handle_sse_event_sync(Ok(sse_message(&content.to_string())), &mut state).unwrap();
    events.extend(handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap());

    assert_eq!(message_text(&events), "Test received.");
}

#[test]
fn keeps_leading_blank_lines_when_no_reasoning_precedes_them() {
    let mut state = StreamState::new("test", false);

    let content = json!({
        "choices": [{
            "delta": { "content": "\n\nTest received." },
            "index": 0,
            "finish_reason": null
        }]
    });
    let mut events =
        handle_sse_event_sync(Ok(sse_message(&content.to_string())), &mut state).unwrap();
    events.extend(handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap());

    assert_eq!(message_text(&events), "\n\nTest received.");
}

#[test]
fn strips_a_separator_split_across_frames() {
    let mut state = StreamState::new("test", false);

    let reasoning = json!({
        "choices": [{
            "delta": { "reasoning": "Deciding what to say.\n" },
            "index": 0,
            "finish_reason": null
        }]
    });
    handle_sse_event_sync(Ok(sse_message(&reasoning.to_string())), &mut state).unwrap();

    let mut events = vec![];
    for chunk in ["\n", "\n", "Test received."] {
        let content = json!({
            "choices": [{
                "delta": { "content": chunk },
                "index": 0,
                "finish_reason": null
            }]
        });
        events.extend(
            handle_sse_event_sync(Ok(sse_message(&content.to_string())), &mut state).unwrap(),
        );
    }
    events.extend(handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap());

    assert_eq!(message_text(&events), "Test received.");
}

#[test]
fn keeps_the_indentation_an_answer_opens_with() {
    let mut state = StreamState::new("test", false);

    let reasoning = json!({
        "choices": [{
            "delta": { "reasoning": "They want the body only.\n" },
            "index": 0,
            "finish_reason": null
        }]
    });
    handle_sse_event_sync(Ok(sse_message(&reasoning.to_string())), &mut state).unwrap();

    let content = json!({
        "choices": [{
            "delta": { "content": "\n\n    return value\n" },
            "index": 0,
            "finish_reason": null
        }]
    });
    let mut events =
        handle_sse_event_sync(Ok(sse_message(&content.to_string())), &mut state).unwrap();
    events.extend(handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap());

    assert_eq!(message_text(&events), "    return value\n");
}

#[test]
fn keeps_indentation_that_arrives_after_the_separator() {
    let mut state = StreamState::new("test", false);

    let reasoning = json!({
        "choices": [{
            "delta": { "reasoning": "They want the body only.\n" },
            "index": 0,
            "finish_reason": null
        }]
    });
    handle_sse_event_sync(Ok(sse_message(&reasoning.to_string())), &mut state).unwrap();

    let mut events = vec![];
    for chunk in ["\n\n", "    return value\n"] {
        let content = json!({
            "choices": [{
                "delta": { "content": chunk },
                "index": 0,
                "finish_reason": null
            }]
        });
        events.extend(
            handle_sse_event_sync(Ok(sse_message(&content.to_string())), &mut state).unwrap(),
        );
    }
    events.extend(handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap());

    assert_eq!(message_text(&events), "    return value\n");
}

/// A server that sends its own reasoning has already done the separating, so
/// the answer is left alone.
/// Asking a locally-served model about chat templates gets a literal `<think>`
/// block back; scanning for one here would delete the tags and move the text
/// behind them out of the answer and into the reasoning region.
#[test]
fn keeps_a_literal_think_block_in_an_answer_the_server_separated() {
    let mut state = StreamState::new("test", false);

    let reasoning = json!({
        "choices": [{
            "delta": { "reasoning": "They want the template.\n" },
            "index": 0,
            "finish_reason": null
        }]
    });
    let mut events =
        handle_sse_event_sync(Ok(sse_message(&reasoning.to_string())), &mut state).unwrap();

    let answer = "Qwen renders it as:\n\n```\n<think>\nplan\n</think>\nanswer\n```\n";
    let content = json!({
        "choices": [{
            "delta": { "content": answer },
            "index": 0,
            "finish_reason": null
        }]
    });
    events
        .extend(handle_sse_event_sync(Ok(sse_message(&content.to_string())), &mut state).unwrap());
    events.extend(handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap());

    assert_eq!(message_text(&events), answer);
    assert_eq!(reasoning_text(&events), "They want the template.\n");
}

/// With no reasoning field anywhere in the stream, the server is leaving the
/// tags in the content for us to parse (llama.cpp's `--reasoning-format none`),
/// so the extractor still runs.
#[test]
fn still_extracts_think_tags_when_the_server_sends_no_reasoning_field() {
    let mut state = StreamState::new("test", false);

    let content = json!({
        "choices": [{
            "delta": { "content": "<think>\nLet me reason...\n</think>\nThe answer." },
            "index": 0,
            "finish_reason": null
        }]
    });
    let mut events =
        handle_sse_event_sync(Ok(sse_message(&content.to_string())), &mut state).unwrap();
    events.extend(handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap());

    assert_eq!(reasoning_text(&events), "Let me reason...\n");
    assert_eq!(message_text(&events), "The answer.");
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

#[test_log::test(tokio::test)]
async fn surfaces_an_in_stream_error_instead_of_finishing() {
    // vLLM reports a mid-generation failure this way: the response has already
    // returned 200, so the error arrives as a payload followed by `[DONE]`.
    let content = sse_message(
        r#"{"choices":[{"delta":{"content":"partial"},"index":0,"finish_reason":null}]}"#,
    );
    let error = sse_message(
        r#"{"error":{"message":"Internal server error","type":"InternalServerError","param":null,"code":500}}"#,
    );
    let events = stream::iter(vec![Ok(content), Ok(error), Ok(sse_message("[DONE]"))]);

    let out: Vec<_> = assemble_event_stream(events, "test", false).collect().await;

    let errors: Vec<_> = out.iter().filter_map(|e| e.as_ref().err()).collect();
    assert_eq!(errors.len(), 1, "expected one error, got {out:?}");
    assert_eq!(errors[0].message(), "Internal server error");
    assert!(
        errors[0].is_retryable(),
        "the request was accepted, so a fresh attempt is worth making"
    );
    assert!(
        !out.iter().any(|e| matches!(e, Ok(Event::Finished(_)))),
        "a failed stream must not report a finish, got {out:?}"
    );
}

#[test]
fn an_in_stream_error_drops_pending_tool_calls() {
    let mut state = StreamState::new("test", false);

    // Arguments cut off mid-JSON, as they are when a failure lands while the
    // call is still streaming. Flushing this buffer commits them, and
    // downstream dispatches the call.
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

    let error = r#"{"error":{"message":"Internal server error","code":500}}"#;
    let error_events = handle_sse_event_sync(Ok(sse_message(error)), &mut state).unwrap();
    assert!(
        error_events.iter().all(std::result::Result::is_err),
        "the error frame carries the failure and nothing else, got {error_events:?}"
    );

    let done_events = handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap();

    assert!(
        done_events.is_empty(),
        "[DONE] after a failure must emit nothing, got {done_events:?}"
    );
}

#[test]
fn length_finish_reason_drops_pending_tool_calls() {
    let mut state = StreamState::new("test", false);

    // Tool call delta with partial arguments, as the model leaves them when it
    // hits the token limit. Committing these parses the truncated JSON down to
    // `{}` and re-dispatches the call with no arguments at all.
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

#[test]
fn tool_call_frame_releases_extractor_tail_before_tool_call_parts() {
    let mut state = StreamState::new("test", false);

    // A full paragraph in one frame, ending in a word long enough that the
    // hold-back window splits it. Released after the tool-call parts instead,
    // the tail lands in a fresh paragraph and renders as `…directo`, a blank
    // line, then `ries.`.
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

#[test]
fn parse_chunk_accepts_an_ordinary_chunk() {
    let data = r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#;

    let chunk = parse_chunk(data, "test")
        .expect("an ordinary chunk is not an error")
        .expect("chunk with choices is kept");

    assert_eq!(chunk.choices.len(), 1);
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
}

#[test]
fn parse_chunk_drops_a_malformed_payload() {
    assert!(
        parse_chunk("{not json", "test")
            .expect("a malformed payload is not reported as a provider error")
            .is_none()
    );
}

#[test]
fn parse_chunk_drops_a_chunk_without_choices() {
    // The benign shape: a server reporting usage in a chunk of its own sends
    // one, and it is not worth a warning.
    let data = r#"{"choices":[],"usage":{"total_tokens":7}}"#;

    assert!(parse_chunk(data, "test").expect("not an error").is_none());
}

#[test]
fn parse_chunk_drops_a_role_only_chunk() {
    // llama.cpp opens every stream with one of these and repeats it on each
    // progress update. It carries a choice, but nothing a handler can emit.
    let data = r#"{"choices":[{"finish_reason":null,"index":0,"delta":{"role":"assistant","content":null}}]}"#;

    assert!(parse_chunk(data, "test").expect("not an error").is_none());
}

#[test]
fn parse_chunk_captures_an_in_stream_error() {
    // The chunk types ignore unknown fields, so an uncaptured `error` would
    // deserialize into a chunk with empty `choices` and be indistinguishable
    // from a usage-only chunk.
    let data = r#"{"error":{"message":"upstream exploded","type":"server_error"}}"#;

    assert_eq!(
        parse_chunk(data, "test").unwrap_err(),
        json!({"message":"upstream exploded","type":"server_error"})
    );
}

#[test]
fn stream_error_message_reads_the_message_field() {
    let payload = json!({
        "message": "Internal server error",
        "type": "InternalServerError",
        "code": 500,
    });

    assert_eq!(stream_error_message(&payload), "Internal server error");
}

#[test]
fn stream_error_message_falls_back_to_the_whole_payload() {
    let payload = json!({ "detail": "out of memory" });

    assert_eq!(
        stream_error_message(&payload),
        r#"{"detail":"out of memory"}"#
    );
}

#[test]
fn an_ordinary_chunk_carries_no_error() {
    let data = r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#;

    let chunk: StreamChunk = serde_json::from_str(data).expect("chunk parses");

    assert_eq!(chunk.error, None);
}
