use assert_matches::assert_matches;
use serde_json::json;

use super::*;
use crate::EventKind;

#[test]
fn test_accumulates_reasoning_chunks() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "Hello ".into(),
        }),
    );
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "world".into(),
        }),
    );
    let event = builder.handle_flush(0, IndexMap::new()).unwrap();

    assert_matches!(
        &event.kind,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
            if reasoning == "Hello world"
    );
}

#[test]
fn test_accumulates_message_chunks() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        1,
        ConversationEvent::now(ChatResponse::Message {
            message: "Hello ".into(),
        }),
    );
    builder.handle_part(
        1,
        ConversationEvent::now(ChatResponse::Message {
            message: "world".into(),
        }),
    );
    let event = builder.handle_flush(1, IndexMap::new()).unwrap();

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

    let request = ToolCallRequest {
        id: "call_1".into(),
        name: "test_tool".into(),
        arguments: serde_json::Map::new(),
    };

    builder.handle_part(2, ConversationEvent::now(request));
    let event = builder.handle_flush(2, IndexMap::new()).unwrap();

    let req = event.as_tool_call_request().expect("expected a tool call");
    assert_eq!(req.name, "test_tool");
}

#[test]
fn test_merges_multi_part_tool_call() {
    let mut builder = EventBuilder::new();

    // First Part: name + id, empty arguments (from content_block_start)
    builder.handle_part(
        1,
        ConversationEvent::now(ToolCallRequest {
            id: "call_42".into(),
            name: "fs_create_file".into(),
            arguments: serde_json::Map::new(),
        }),
    );

    // Second Part: arguments only (from content_block_stop after JSON
    // aggregation)
    let mut args = serde_json::Map::new();
    args.insert("path".into(), "src/main.rs".into());
    args.insert("content".into(), "fn main() {}".into());
    builder.handle_part(
        1,
        ConversationEvent::now(ToolCallRequest {
            id: "call_42".into(),
            name: "fs_create_file".into(),
            arguments: args,
        }),
    );

    let event = builder.handle_flush(1, IndexMap::new()).unwrap();

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
        ConversationEvent::now(ToolCallRequest {
            id: "first_id".into(),
            name: "first_name".into(),
            arguments: serde_json::Map::new(),
        }),
    );

    // Second Part with different id+name (should be ignored for id/name)
    let mut args = serde_json::Map::new();
    args.insert("key".into(), "value".into());
    builder.handle_part(
        0,
        ConversationEvent::now(ToolCallRequest {
            id: "second_id".into(),
            name: "second_name".into(),
            arguments: args,
        }),
    );

    let event = builder.handle_flush(0, IndexMap::new()).unwrap();

    let req = event.as_tool_call_request().expect("expected a tool call");

    // First non-empty wins
    assert_eq!(req.id, "first_id");
    assert_eq!(req.name, "first_name");
    // Arguments are extended
    assert_eq!(req.arguments["key"], "value");
}

#[test]
fn test_interleaved_indices() {
    let mut builder = EventBuilder::new();

    // Index 0: Message
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Message {
            message: "Part 1".into(),
        }),
    );
    // Index 1: Reasoning
    builder.handle_part(
        1,
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "Reasoning".into(),
        }),
    );
    // Index 0: Message continues
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Message {
            message: " Part 2".into(),
        }),
    );

    // Flush 1 first
    let event1 = builder.handle_flush(1, IndexMap::new()).unwrap();
    // Flush 0 second
    let event2 = builder.handle_flush(0, IndexMap::new()).unwrap();

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

    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Message {
            message: "Hello".into(),
        }),
    );

    let mut metadata = IndexMap::new();
    metadata.insert("tokens".to_string(), Value::Number(100.into()));

    let event = builder.handle_flush(0, metadata).unwrap();

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
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "Let me think...".into(),
        }),
    );

    // Second part: empty reasoning content with signature metadata
    // (simulates Anthropic's SignatureDelta).
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: String::new(),
        })
        .with_metadata_field("anthropic_thinking_signature", "sig_abc123"),
    );

    // Flush with no additional metadata.
    let event = builder.handle_flush(0, IndexMap::new()).unwrap();

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
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "thinking".into(),
        })
        .with_metadata_field("from_part", "part_value"),
    );

    let mut flush_metadata = IndexMap::new();
    flush_metadata.insert(
        "from_flush".to_string(),
        Value::String("flush_value".into()),
    );

    let event = builder.handle_flush(0, flush_metadata).unwrap();

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

    // Simulate Anthropic emitting "\n\n" as a text content block
    // between interleaved thinking blocks.
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Message {
            message: "\n\n".into(),
        }),
    );
    assert!(builder.handle_flush(0, IndexMap::new()).is_none());
}

#[test]
fn test_ignores_mismatched_event_type() {
    let mut builder = EventBuilder::new();

    // Start with Reasoning
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "Thinking...".into(),
        }),
    );

    // Try to append Message to same index (should be ignored)
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Message {
            message: "Hello".into(),
        }),
    );

    let event = builder.handle_flush(0, IndexMap::new()).unwrap();
    assert_matches!(
        &event.kind,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
            if reasoning == "Thinking..."
    );
}

#[test]
fn test_ignores_irrelevant_event_kinds() {
    let mut builder = EventBuilder::new();

    // ChatRequest should be ignored
    builder.handle_part(
        0,
        ConversationEvent::now(crate::EventKind::ChatRequest(crate::event::ChatRequest {
            content: String::new(),
            schema: None,
        })),
    );

    // Flush should produce nothing because nothing was buffered
    assert!(builder.handle_flush(0, IndexMap::new()).is_none());
}

#[test]
fn test_peek_partial_content_empty() {
    let builder = EventBuilder::new();
    assert_eq!(builder.peek_partial_content(), None);
}

#[test]
fn test_peek_partial_content_single_buffer() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Message {
            message: "Hello ".into(),
        }),
    );
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Message {
            message: "world".into(),
        }),
    );

    assert_eq!(
        builder.peek_partial_content(),
        Some("Hello world".to_string())
    );
}

#[test]
fn test_peek_partial_content_multiple_buffers() {
    let mut builder = EventBuilder::new();

    // Index 0: Reasoning
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "Let me think".into(),
        }),
    );
    // Index 1: Message
    builder.handle_part(
        1,
        ConversationEvent::now(ChatResponse::Message {
            message: "The answer is".into(),
        }),
    );

    // Should concatenate in index order
    assert_eq!(
        builder.peek_partial_content(),
        Some("Let me thinkThe answer is".to_string())
    );
}

#[test]
fn test_peek_partial_content_after_partial_flush() {
    let mut builder = EventBuilder::new();

    // Index 0: Reasoning (will be flushed)
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Reasoning {
            reasoning: "Thinking...".into(),
        }),
    );
    // Index 1: Message (will remain unflushed)
    builder.handle_part(
        1,
        ConversationEvent::now(ChatResponse::Message {
            message: "Partial answer".into(),
        }),
    );

    // Flush index 0 only
    builder.handle_flush(0, IndexMap::new());

    // Only index 1 should remain
    assert_eq!(
        builder.peek_partial_content(),
        Some("Partial answer".to_string())
    );
}

// --- Structured response (Phase 2) ---

#[test]
fn test_accumulates_structured_chunks() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Structured {
            data: Value::String("{\"name".into()),
        }),
    );
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Structured {
            data: Value::String("\": \"Alice\"}".into()),
        }),
    );
    let event = builder.handle_flush(0, IndexMap::new()).unwrap();

    let resp = event.as_chat_response().unwrap();
    assert_eq!(resp.as_structured_data(), Some(&json!({"name": "Alice"})));
}

#[test]
fn test_structured_malformed_json_falls_back_to_string() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Structured {
            data: Value::String("{\"truncated".into()),
        }),
    );
    let event = builder.handle_flush(0, IndexMap::new()).unwrap();

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
        ConversationEvent::now(ChatResponse::Structured {
            data: Value::String("{}".into()),
        })
        .with_metadata_field("provider", "anthropic"),
    );

    let mut flush_meta = IndexMap::new();
    flush_meta.insert("tokens".into(), json!(42));
    let event = builder.handle_flush(0, flush_meta).unwrap();

    assert_eq!(
        event.metadata.get("provider"),
        Some(&Value::String("anthropic".into()))
    );
    assert_eq!(event.metadata.get("tokens"), Some(&json!(42)));
}

#[test]
fn test_structured_ignores_non_string_part() {
    let mut builder = EventBuilder::new();

    // A part with a non-string Value should be silently ignored.
    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Structured {
            data: json!({"already": "parsed"}),
        }),
    );
    assert!(builder.handle_flush(0, IndexMap::new()).is_none());
}

#[test]
fn test_structured_not_included_in_peek_partial_content() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Structured {
            data: Value::String("{\"partial".into()),
        }),
    );

    // Structured buffers are not useful for assistant prefill.
    assert_eq!(builder.peek_partial_content(), None);
}

#[test]
fn test_structured_array_response() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        ConversationEvent::now(ChatResponse::Structured {
            data: Value::String("[\"title one\",\"title two\"]".into()),
        }),
    );
    let event = builder.handle_flush(0, IndexMap::new()).unwrap();

    let resp = event.as_chat_response().unwrap();
    assert_eq!(
        resp.as_structured_data(),
        Some(&json!(["title one", "title two"]))
    );
}
