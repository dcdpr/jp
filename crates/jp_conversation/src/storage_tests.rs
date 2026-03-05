use serde_json::json;

use super::*;

#[test]
fn encode_decode_tool_call_request() {
    let mut value = json!({
        "type": "tool_call_request",
        "id": "tc_1",
        "name": "read_file",
        "arguments": {"path": "src/main.rs"},
        "metadata": {"model": "gpt-4"}
    });

    let kind = EventKind::ToolCallRequest(crate::event::ToolCallRequest::new(
        String::new(),
        String::new(),
        serde_json::Map::default(),
    ));

    encode_event(&mut value, &kind);

    // Structural fields are untouched.
    assert_eq!(value["id"], "tc_1");
    assert_eq!(value["name"], "read_file");

    // Content fields are encoded.
    assert_ne!(value["arguments"]["path"], "src/main.rs");
    assert_ne!(value["metadata"]["model"], "gpt-4");

    // Round-trip.
    decode_event_value(&mut value);
    assert_eq!(value["arguments"]["path"], "src/main.rs");
    assert_eq!(value["metadata"]["model"], "gpt-4");
}

#[test]
fn encode_decode_tool_call_response() {
    let mut value = json!({
        "type": "tool_call_response",
        "id": "tc_1",
        "content": "file contents here",
        "is_error": false
    });

    let kind = EventKind::ToolCallResponse(crate::event::ToolCallResponse {
        id: String::new(),
        result: Ok(String::new()),
    });

    encode_event(&mut value, &kind);
    assert_ne!(value["content"], "file contents here");
    assert_eq!(value["id"], "tc_1");

    decode_event_value(&mut value);
    assert_eq!(value["content"], "file contents here");
}

#[test]
fn events_without_content_fields_are_unchanged() {
    let original = json!({
        "type": "chat_request",
        "content": "hello world",
        "timestamp": "2025-01-01T00:00:00Z"
    });
    let mut value = original.clone();

    let kind = EventKind::ChatRequest(crate::event::ChatRequest::from("hello world"));
    encode_event(&mut value, &kind);

    assert_eq!(value["content"], "hello world");

    decode_event_value(&mut value);
    assert_eq!(value, original);
}
