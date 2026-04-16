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
    let event = builder.handle_flush(0, Map::new()).unwrap();

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
    let event = builder.handle_flush(1, Map::new()).unwrap();

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
        }),
        Map::new(),
    );
    let event = builder.handle_flush(2, Map::new()).unwrap();

    let req = event.as_tool_call_request().expect("expected a tool call");
    assert_eq!(req.name, "test_tool");
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

    let event = builder.handle_flush(1, Map::new()).unwrap();

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
        }),
        Map::new(),
    );

    // Second Part with different id+name (should be ignored for id/name)
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::Start {
            id: "second_id".into(),
            name: "second_name".into(),
        }),
        Map::new(),
    );

    // Arguments arrive separately
    builder.handle_part(
        0,
        EventPart::ToolCall(ToolCallPart::ArgumentChunk(r#"{"key": "value"}"#.into())),
        Map::new(),
    );

    let event = builder.handle_flush(0, Map::new()).unwrap();

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
    let event1 = builder.handle_flush(1, Map::new()).unwrap();
    // Flush 0 second
    let event2 = builder.handle_flush(0, Map::new()).unwrap();

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
    let event = builder.handle_flush(0, Map::new()).unwrap();

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

    builder.handle_part(0, EventPart::Message("\n\n".into()), Map::new());
    assert!(builder.handle_flush(0, Map::new()).is_none());
}

#[test]
fn test_ignores_mismatched_event_type() {
    let mut builder = EventBuilder::new();

    // Start with Reasoning
    builder.handle_part(0, EventPart::Reasoning("Thinking...".into()), Map::new());

    // Try to append Message to same index (should be ignored)
    builder.handle_part(0, EventPart::Message("Hello".into()), Map::new());

    let event = builder.handle_flush(0, Map::new()).unwrap();
    assert_matches!(
        &event.kind,
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning })
            if reasoning == "Thinking..."
    );
}

#[test]
fn test_peek_partial_content_empty() {
    let builder = EventBuilder::new();
    assert_eq!(builder.peek_partial_content(), None);
}

#[test]
fn test_peek_partial_content_single_buffer() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Message("Hello ".into()), Map::new());
    builder.handle_part(0, EventPart::Message("world".into()), Map::new());

    assert_eq!(
        builder.peek_partial_content(),
        Some("Hello world".to_string())
    );
}

#[test]
fn test_peek_partial_content_multiple_buffers() {
    let mut builder = EventBuilder::new();

    // Index 0: Reasoning
    builder.handle_part(0, EventPart::Reasoning("Let me think".into()), Map::new());
    // Index 1: Message
    builder.handle_part(1, EventPart::Message("The answer is".into()), Map::new());

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
    builder.handle_part(0, EventPart::Reasoning("Thinking...".into()), Map::new());
    // Index 1: Message (will remain unflushed)
    builder.handle_part(1, EventPart::Message("Partial answer".into()), Map::new());

    // Flush index 0 only
    builder.handle_flush(0, Map::new());

    // Only index 1 should remain
    assert_eq!(
        builder.peek_partial_content(),
        Some("Partial answer".to_string())
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
    let event = builder.handle_flush(0, Map::new()).unwrap();

    let resp = event.as_chat_response().unwrap();
    assert_eq!(resp.as_structured_data(), Some(&json!({"name": "Alice"})));
}

#[test]
fn test_structured_malformed_json_falls_back_to_string() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Structured("{\"truncated".into()), Map::new());
    let event = builder.handle_flush(0, Map::new()).unwrap();

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
    let event = builder.handle_flush(0, flush_meta).unwrap();

    assert_eq!(
        event.metadata.get("provider"),
        Some(&Value::String("anthropic".into()))
    );
    assert_eq!(event.metadata.get("tokens"), Some(&json!(42)));
}

#[test]
fn test_structured_not_included_in_peek_partial_content() {
    let mut builder = EventBuilder::new();

    builder.handle_part(0, EventPart::Structured("{\"partial".into()), Map::new());

    // Structured buffers are not useful for assistant prefill.
    assert_eq!(builder.peek_partial_content(), None);
}

#[test]
fn test_structured_array_response() {
    let mut builder = EventBuilder::new();

    builder.handle_part(
        0,
        EventPart::Structured("[\"title one\",\"title two\"]".into()),
        Map::new(),
    );
    let event = builder.handle_flush(0, Map::new()).unwrap();

    let resp = event.as_chat_response().unwrap();
    assert_eq!(
        resp.as_structured_data(),
        Some(&json!(["title one", "title two"]))
    );
}

/// Reproduces the production bug and proves `AutoEscape::Json` fixes it.
///
/// Without JSON auto-escaping, minijinja renders null values as `none`
/// (Jinja2 convention). `serde_json` then rejects the output with
/// `"expected ident"` because `n` starts the `null` parser but `o`
/// doesn't match `u`. With `AutoEscape::Json`, values are serialized
/// as proper JSON (`null`, not `none`).
#[test]
fn test_json_auto_escape_fixes_null_rendering() {
    use minijinja::{AutoEscape, Environment};

    let arguments: Map<String, Value> =
        serde_json::from_str(r#"{"package": "jp_workspace", "backtrace": null}"#).unwrap();

    let ctx = json!({
        "tool": {
            "name": "cargo_test",
            "arguments": arguments,
            "answers": {},
            "options": {},
        },
    });

    // Default environment: produces `none` for null (the bug).
    let default_env = Environment::new();
    let broken = default_env.render_str("{{tool}}", &ctx).unwrap();
    assert!(
        broken.contains("none"),
        "Default env should render null as 'none': {broken}"
    );
    let err = serde_json::from_str::<Value>(&broken).unwrap_err();
    assert!(
        err.to_string().contains("expected ident"),
        "Expected 'expected ident' error, got: {err}"
    );

    // JSON auto-escape environment: produces valid JSON (the fix).
    let mut json_env = Environment::new();
    json_env.set_auto_escape_callback(|_| AutoEscape::Json);
    let fixed = json_env.render_str("{{tool}}", &ctx).unwrap();
    assert!(
        !fixed.contains("none"),
        "JSON env should not render 'none': {fixed}"
    );
    let parsed: Value = serde_json::from_str(&fixed).unwrap_or_else(|e| {
        panic!("JSON env output should be valid JSON: {e}\n\nOutput: {fixed}")
    });
    assert_eq!(parsed["arguments"]["backtrace"], Value::Null);
    assert_eq!(parsed["arguments"]["package"], "jp_workspace");
}
