use assert_matches::assert_matches;
use jp_conversation::{EventKind, event::ChatResponse};
use serde_json::{Map, Value, json};

use super::*;
use crate::event::{EventPart, ToolCallPart};

#[test]
fn test_accumulates_reasoning_chunks() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Reasoning("Hello ".into()), Map::new());
    builder.handle_part(0, EventPart::Reasoning("world".into()), Map::new());
    let event = builder.handle_flush(0, Map::new()).unwrap().unwrap();

    assert_matches!(
        &event.kind,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
            if reasoning == "Hello world"
    );
}

#[test]
fn test_accumulates_message_chunks() {
    let mut builder = EventBuilder::new();

    builder.handle_part(1, EventPart::Message("Hello ".into()), Map::new());
    builder.handle_part(1, EventPart::Message("world".into()), Map::new());
    let event = builder.handle_flush(1, Map::new()).unwrap().unwrap();

    match &event.kind {
        EventKind::ChatResponse(ChatResponse::Message { message }) => {
            assert_eq!(message, "Hello world");
        }
        _ => panic!("Expected message event"),
    }
}

#[test]
fn test_handles_tool_call() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        2,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "call_1".into(),
            name: "test_tool".into(),
            decoding: None,
        }),
        Map::new(),
    );
    let event = builder.handle_flush(2, Map::new()).unwrap().unwrap();

    let req = event.as_tool_call_request().expect("expected a tool call");
    assert_eq!(req.name, "test_tool");
}

/// A malformed stream can open a tool-call buffer from an argument chunk that
/// arrives before any `Start`, leaving the name empty.
/// Those unnamed buffers must not appear in the diagnostic, where they would
/// render as empty names.
#[test]
fn test_incomplete_tool_calls_skips_unnamed_buffers() {
    let mut builder = EventBuilder::new();

    // A real, named tool call still mid-stream (no flush yet).
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            decoding: None,
        }),
        Map::new(),
    );

    // A malformed buffer: an argument chunk before any `Start`, so its name and
    // id are both empty.
    builder.handle_part(
        1,
        EventPart::ToolCall(ToolCallPart::ArgumentChunk("{}".into())),
        Map::new(),
    );

    assert_eq!(builder.incomplete_tool_calls(), vec![
        "fs_modify_file".to_string()
    ]);
}

#[test]
fn test_merges_multi_part_tool_call() {
    let mut builder = EventBuilder::new();

    // First Part: name + id (from content_block_start)
    builder.handle_part(
        1,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "call_42".into(),
            name: "fs_create_file".into(),
            decoding: None,
        }),
        Map::new(),
    );

    // Argument chunks
    builder.handle_part(
        1,
        EventPart::ToolCall(ToolCallPart::ArgumentChunk(
            r#"{"path": "src/main.rs", "content": "fn main() {}"}"#.into(),
        )),
        Map::new(),
    );

    let event = builder.handle_flush(1, Map::new()).unwrap().unwrap();

    let req = event.as_tool_call_request().expect("expected a tool call");
    assert_eq!(req.id, "call_42");
    assert_eq!(req.name, "fs_create_file");
    assert_eq!(req.arguments.len(), 2);
    assert_eq!(req.arguments["path"], "src/main.rs");
    assert_eq!(req.arguments["content"], "fn main() {}");
}

#[test]
fn test_multi_part_tool_call_first_write_wins_for_id_and_name() {
    let mut builder = EventBuilder::new();

    // First Part with id+name
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "first_id".into(),
            name: "first_name".into(),
            decoding: None,
        }),
        Map::new(),
    );

    // Second Part with different id+name (should be ignored for id/name)
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "second_id".into(),
            name: "second_name".into(),
            decoding: None,
        }),
        Map::new(),
    );

    // Arguments arrive separately
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::ArgumentChunk(r#"{"key": "value"}"#.into())),
        Map::new(),
    );

    let event = builder.handle_flush(0, Map::new()).unwrap().unwrap();

    let req = event.as_tool_call_request().expect("expected a tool call");

    // First non-empty wins
    assert_eq!(req.id, "first_id");
    assert_eq!(req.name, "first_name");
    // Arguments are parsed
    assert_eq!(req.arguments["key"], "value");
}

#[test]
fn test_interleaved_indices() {
    let mut builder = EventBuilder::new();

    // Index 0: Message
    builder.handle_part(0, EventPart::Message("Part 1".into()), Map::new());
    // Index 1: Reasoning
    builder.handle_part(1, EventPart::Reasoning("Reasoning".into()), Map::new());
    // Index 0: Message continues
    builder.handle_part(0, EventPart::Message(" Part 2".into()), Map::new());

    // Flush 1 first
    let event1 = builder.handle_flush(1, Map::new()).unwrap().unwrap();
    // Flush 0 second
    let event2 = builder.handle_flush(0, Map::new()).unwrap().unwrap();

    assert_matches!(
        &event1.kind,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
            if reasoning == "Reasoning"
    );
    assert_matches!(
        &event2.kind,
        EventKind::ChatResponse(ChatResponse::Message { message })
            if message == "Part 1 Part 2"
    );
}

#[test]
fn test_metadata_preserved_on_flush() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Message("Hello".into()), Map::new());

    let mut metadata = Map::new();
    metadata.insert("tokens".to_string(), Value::Number(100.into()));

    let event = builder.handle_flush(0, metadata).unwrap().unwrap();

    assert_eq!(
        event.metadata.get("tokens"),
        Some(&Value::Number(100.into()))
    );
}

/// Regression test: metadata arriving on individual `Part` events (e.g.
/// Anthropic thinking signatures via `SignatureDelta`) must be preserved
/// through aggregation and appear on the flushed event.
#[test]
fn test_part_metadata_accumulated_through_flush() {
    let mut builder = EventBuilder::new();

    // First part: reasoning content, no metadata.
    builder.handle_part(
        0,
        EventPart::Reasoning("Let me think...".into()),
        Map::new(),
    );

    // Second part: empty reasoning content with signature metadata
    // (simulates Anthropic's SignatureDelta).
    builder.handle_part(
        0,
        EventPart::Reasoning(String::new()),
        Map::from_iter([(
            "anthropic_thinking_signature".to_owned(),
            json!("sig_abc123"),
        )]),
    );

    // Flush with no additional metadata.
    let event = builder.handle_flush(0, Map::new()).unwrap().unwrap();

    // Content should be accumulated.
    assert_matches!(
        &event.kind,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
            if reasoning == "Let me think..."
    );

    // Signature metadata from the Part should be present.
    assert_eq!(
        event.metadata.get("anthropic_thinking_signature"),
        Some(&Value::String("sig_abc123".into()))
    );
}

/// Both part metadata and flush metadata should be merged.
#[test]
fn test_part_and_flush_metadata_merged() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        EventPart::Reasoning("thinking".into()),
        Map::from_iter([("from_part".to_owned(), json!("part_value"))]),
    );

    let mut flush_metadata = Map::new();
    flush_metadata.insert(
        "from_flush".to_string(),
        Value::String("flush_value".into()),
    );

    let event = builder.handle_flush(0, flush_metadata).unwrap().unwrap();

    assert_eq!(
        event.metadata.get("from_part"),
        Some(&Value::String("part_value".into()))
    );
    assert_eq!(
        event.metadata.get("from_flush"),
        Some(&Value::String("flush_value".into()))
    );
}

#[test]
fn test_whitespace_only_message_not_persisted() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Message("\n\n".into()), Map::new());
    assert_eq!(builder.handle_flush(0, Map::new()), Ok(None));
}

#[test]
fn test_ignores_mismatched_event_type() {
    let mut builder = EventBuilder::new();

    // Start with Reasoning
    builder.handle_part(0, EventPart::Reasoning("Thinking...".into()), Map::new());

    // Try to append Message to same index (should be ignored)
    builder.handle_part(0, EventPart::Message("Hello".into()), Map::new());

    let event = builder.handle_flush(0, Map::new()).unwrap().unwrap();
    assert_matches!(
        &event.kind,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
            if reasoning == "Thinking..."
    );
}

#[test]
fn test_peek_partial_events_empty() {
    let builder = EventBuilder::new();
    assert_eq!(builder.peek_partial_events(), [] as [ChatResponse; 0]);
}

#[test]
fn test_peek_partial_events_single_message() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Message("Hello ".into()), Map::new());
    builder.handle_part(0, EventPart::Message("world".into()), Map::new());

    assert_matches!(
        builder.peek_partial_events().as_slice(),
        [ChatResponse::Message { message }] if message == "Hello world"
    );
}

/// Partial reasoning is preserved as a `Reasoning` response, ordered before a
/// later message buffer by stream index.
#[test]
fn test_peek_partial_events_includes_reasoning() {
    let mut builder = EventBuilder::new();

    // Index 0: Reasoning
    builder.handle_part(0, EventPart::Reasoning("Let me think".into()), Map::new());
    // Index 1: Message
    builder.handle_part(1, EventPart::Message("The answer is".into()), Map::new());

    assert_matches!(
        builder.peek_partial_events().as_slice(),
        [
            ChatResponse::Reasoning { reasoning },
            ChatResponse::Message { message },
        ] if reasoning == "Let me think" && message == "The answer is"
    );
}

#[test]
fn test_peek_partial_events_after_partial_flush() {
    let mut builder = EventBuilder::new();

    // Index 0: Reasoning (will be flushed)
    builder.handle_part(0, EventPart::Reasoning("Thinking...".into()), Map::new());
    // Index 1: Message (will remain unflushed)
    builder.handle_part(1, EventPart::Message("Partial answer".into()), Map::new());

    // Flush index 0 only
    builder.handle_flush(0, Map::new()).unwrap();

    // Only index 1 should remain
    assert_matches!(
        builder.peek_partial_events().as_slice(),
        [ChatResponse::Message { message }] if message == "Partial answer"
    );
}

#[test]
fn test_accumulates_structured_chunks() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Structured("{\"name".into()), Map::new());
    builder.handle_part(
        0,
        EventPart::Structured("\": \"Alice\"}".into()),
        Map::new(),
    );
    let event = builder.handle_flush(0, Map::new()).unwrap().unwrap();

    let resp = event.as_chat_response().unwrap();
    assert_eq!(resp.as_structured_data(), Some(&json!({"name": "Alice"})));
}

#[test]
fn test_structured_malformed_json_falls_back_to_string() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Structured("{\"truncated".into()), Map::new());
    let event = builder.handle_flush(0, Map::new()).unwrap().unwrap();

    let resp = event.as_chat_response().unwrap();
    assert_eq!(
        resp.as_structured_data(),
        Some(&Value::String("{\"truncated".into()))
    );
}

#[test]
fn test_structured_preserves_metadata() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        EventPart::Structured("{}".into()),
        Map::from_iter([("provider".to_owned(), json!("anthropic"))]),
    );

    let mut flush_meta = Map::new();
    flush_meta.insert("tokens".into(), json!(42));
    let event = builder.handle_flush(0, flush_meta).unwrap().unwrap();

    assert_eq!(
        event.metadata.get("provider"),
        Some(&Value::String("anthropic".into()))
    );
    assert_eq!(event.metadata.get("tokens"), Some(&json!(42)));
}

#[test]
fn test_structured_not_included_in_peek_partial_events() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Structured("{\"partial".into()), Map::new());

    // Structured buffers are not a usable resume point.
    assert_eq!(builder.peek_partial_events(), [] as [ChatResponse; 0]);
}

/// Drain emits buffered text for `Reasoning`/`Message`/`Structured` so partial
/// output isn't lost on stream end, but it MUST drop tool-call buffers — those
/// only reach drain if the explicit `Flush` (Anthropic `ContentBlockStop`)
/// never arrived, which means the tool call is structurally incomplete.
/// Persisting it would create an orphaned `tool_use` block that providers
/// reject on the next request.
#[test]
fn drain_drops_incomplete_tool_call_but_keeps_partial_text() {
    let mut builder = EventBuilder::new();

    // A tool call that received Start but no ArgumentChunk and no Flush.
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "toolu_incomplete".into(),
            name: "some_tool".into(),
            decoding: None,
        }),
        Map::new(),
    );

    // A reasoning buffer that's also unflushed — partial reasoning that
    // should be preserved.
    builder.handle_part(1, EventPart::Reasoning("thinking…".into()), Map::new());

    // A message buffer with partial content — should be preserved.
    builder.handle_part(2, EventPart::Message("halfway through".into()), Map::new());

    let drained = builder.drain();

    // Tool call dropped, text preserved.
    assert_eq!(drained.len(), 2, "expected 2 events, got {drained:?}");
    assert!(
        drained.iter().all(|e| e.as_tool_call_request().is_none()),
        "drain must not emit tool-call requests for unflushed buffers",
    );

    let kinds: Vec<_> = drained.iter().map(|e| &e.kind).collect();
    assert!(matches!(
        kinds.as_slice(),
        [
            EventKind::ChatResponse(ChatResponse::Reasoning { .. }),
            EventKind::ChatResponse(ChatResponse::Message { .. }),
        ] | [
            EventKind::ChatResponse(ChatResponse::Message { .. }),
            EventKind::ChatResponse(ChatResponse::Reasoning { .. }),
        ]
    ));
}

fn flush_tool_call(arguments: &str) -> Result<Option<ConversationEvent>, InvalidToolCall> {
    let mut builder = EventBuilder::new();
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            decoding: None,
        }),
        Map::new(),
    );
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::ArgumentChunk(arguments.into())),
        Map::new(),
    );

    builder.handle_flush(0, Map::new())
}

/// When a provider stops generating mid-call, it still flushes the tool-call
/// block; the truncated JSON must be rejected rather than treated as empty
/// arguments.
#[test]
fn truncated_tool_call_arguments_are_rejected() {
    assert_eq!(
        flush_tool_call(r#"{"path": "src/main.rs", "content": "fn ma"#),
        Err(InvalidToolCall {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            arguments_json: r#"{"path": "src/main.rs", "content": "fn ma"#.into(),
            reason: InvalidArguments::Truncated(
                "EOF while parsing a string at line 1 column 41".into()
            ),
            metadata: Map::new(),
        })
    );
}

#[test]
fn malformed_tool_call_arguments_are_rejected() {
    assert_eq!(
        flush_tool_call(r#"{"path": "a.rs"}{"path": "b.rs"}"#),
        Err(InvalidToolCall {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            arguments_json: r#"{"path": "a.rs"}{"path": "b.rs"}"#.into(),
            reason: InvalidArguments::Malformed("trailing characters at line 1 column 17".into()),
            metadata: Map::new(),
        })
    );
}

#[test]
fn non_object_tool_call_arguments_are_rejected() {
    assert_eq!(
        flush_tool_call(r#"["src/main.rs"]"#),
        Err(InvalidToolCall {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            arguments_json: r#"["src/main.rs"]"#.into(),
            reason: InvalidArguments::NotAnObject("an array"),
            metadata: Map::new(),
        })
    );
}

/// Literal `"null"` is rejected as a non-object rather than treated as empty
/// arguments; providers whose SDK represents missing arguments as `Value::Null`
/// normalize it to `"{}"` before emitting `ArgumentChunk`.
#[test]
fn null_tool_call_arguments_are_rejected() {
    assert_eq!(
        flush_tool_call("null"),
        Err(InvalidToolCall {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            arguments_json: "null".into(),
            reason: InvalidArguments::NotAnObject("null"),
            metadata: Map::new(),
        })
    );
}

#[test]
fn empty_object_tool_call_arguments_are_accepted() {
    let event = flush_tool_call("{}").unwrap().unwrap();

    assert_eq!(
        event.as_tool_call_request(),
        Some(&ToolCallRequest {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            arguments: Map::new(),
        })
    );
}

/// `InvalidToolCall` preserves metadata from both `Part` and `Flush` events so
/// the caller can attach provider metadata (such as thought signatures) to the
/// recorded request.
#[test]
fn rejected_tool_call_keeps_part_and_flush_metadata() {
    let mut builder = EventBuilder::new();
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            decoding: None,
        }),
        Map::from_iter([("from_part".to_owned(), json!("part_value"))]),
    );
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::ArgumentChunk(r#"{"path""#.into())),
        Map::new(),
    );

    let rejected = builder
        .handle_flush(
            0,
            Map::from_iter([("from_flush".to_owned(), json!("flush_value"))]),
        )
        .unwrap_err();

    assert_eq!(
        Value::Object(rejected.metadata),
        json!({"from_part": "part_value", "from_flush": "flush_value"})
    );
}

/// A rejection consumes the buffer like any flush, so a provider that flushes
/// the index twice does not have the call rejected twice.
#[test]
fn duplicate_flush_after_a_rejected_tool_call_returns_nothing() {
    let mut builder = EventBuilder::new();
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "call_1".into(),
            name: "fs_modify_file".into(),
            decoding: None,
        }),
        Map::new(),
    );
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::ArgumentChunk(r#"{"path""#.into())),
        Map::new(),
    );

    let rejected = builder.handle_flush(0, Map::new()).unwrap_err();
    assert_eq!(
        rejected.reason,
        InvalidArguments::Truncated("EOF while parsing an object at line 1 column 7".into())
    );

    assert_eq!(builder.handle_flush(0, Map::new()), Ok(None));
}

#[test]
fn test_structured_array_response() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        EventPart::Structured("[\"title one\",\"title two\"]".into()),
        Map::new(),
    );
    let event = builder.handle_flush(0, Map::new()).unwrap().unwrap();

    let resp = event.as_chat_response().unwrap();
    assert_eq!(
        resp.as_structured_data(),
        Some(&json!(["title one", "title two"]))
    );
}
