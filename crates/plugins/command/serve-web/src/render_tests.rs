use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn render_empty_events() {
    let events = render_events(&[]);
    assert!(events.is_empty());
}

#[test]
fn render_chat_request_and_response() {
    let events = vec![
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "Hello"}),
        json!({"type": "chat_response", "timestamp": "2025-01-01T00:00:02Z", "message": "Hi there"}),
    ];

    let rendered = render_events(&events);
    assert_eq!(rendered.len(), 2); // no separator for first turn

    assert!(matches!(&rendered[0], RenderedEvent::UserMessage { html } if html.contains("Hello")));
    assert!(
        matches!(&rendered[1], RenderedEvent::AssistantMessage { html } if html.contains("Hi there"))
    );
}

#[test]
fn render_turn_separator() {
    let events = vec![
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "First"}),
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:01:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:01:01Z", "content": "Second"}),
    ];

    let rendered = render_events(&events);
    assert_eq!(rendered.len(), 3);
    assert!(matches!(&rendered[0], RenderedEvent::UserMessage { .. }));
    assert!(matches!(&rendered[1], RenderedEvent::TurnSeparator));
    assert!(matches!(&rendered[2], RenderedEvent::UserMessage { .. }));
}

#[test]
fn render_reasoning() {
    let events = vec![
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "think"}),
        json!({"type": "chat_response", "timestamp": "2025-01-01T00:00:02Z", "reasoning": "Let me think..."}),
        json!({"type": "chat_response", "timestamp": "2025-01-01T00:00:03Z", "message": "Done."}),
    ];

    let rendered = render_events(&events);
    assert_eq!(rendered.len(), 3);
    assert!(
        matches!(&rendered[1], RenderedEvent::Reasoning { html } if html.contains("Let me think"))
    );
}

#[test]
fn render_structured_response() {
    let events = vec![
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "data"}),
        json!({"type": "chat_response", "timestamp": "2025-01-01T00:00:02Z", "data": {"key": "value"}}),
    ];

    let rendered = render_events(&events);
    assert_eq!(rendered.len(), 2);
    assert!(matches!(&rendered[1], RenderedEvent::Structured { json } if json.contains("key")));
}

#[test]
fn render_tool_call_with_response() {
    let events = vec![
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "go"}),
        json!({
            "type": "tool_call_request",
            "timestamp": "2025-01-01T00:00:02Z",
            "id": "tc_1",
            "name": "read_file",
            "arguments": {"path": "test.rs"}
        }),
        json!({
            "type": "tool_call_response",
            "timestamp": "2025-01-01T00:00:03Z",
            "id": "tc_1",
            "content": "file contents here",
            "is_error": false
        }),
    ];

    let rendered = render_events(&events);
    assert_eq!(rendered.len(), 2); // chat_request + tool_call

    match &rendered[1] {
        RenderedEvent::ToolCall {
            name,
            arguments,
            result,
        } => {
            assert_eq!(name, "read_file");
            assert!(arguments.contains("test.rs"));
            assert_eq!(result.as_deref(), Some("file contents here"));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn render_tool_call_without_response() {
    let events = vec![
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "go"}),
        json!({
            "type": "tool_call_request",
            "timestamp": "2025-01-01T00:00:02Z",
            "id": "tc_orphan",
            "name": "some_tool",
            "arguments": {}
        }),
    ];

    let rendered = render_events(&events);
    assert_eq!(rendered.len(), 2);

    match &rendered[1] {
        RenderedEvent::ToolCall { result, .. } => {
            assert!(result.is_none());
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

#[test]
fn config_delta_events_are_skipped() {
    let events = vec![
        json!({"type": "config_delta", "timestamp": "2025-01-01T00:00:00Z", "delta": {}}),
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "hi"}),
    ];

    let rendered = render_events(&events);
    assert_eq!(rendered.len(), 1);
    assert!(matches!(&rendered[0], RenderedEvent::UserMessage { .. }));
}

#[test]
fn plain_text_tool_response() {
    // The host decodes base64 before sending events to the plugin,
    // so content arrives as plain text.
    let events = vec![
        json!({"type": "turn_start", "timestamp": "2025-01-01T00:00:00Z"}),
        json!({"type": "chat_request", "timestamp": "2025-01-01T00:00:01Z", "content": "go"}),
        json!({
            "type": "tool_call_request",
            "timestamp": "2025-01-01T00:00:02Z",
            "id": "tc_1",
            "name": "test",
            "arguments": {"key": "value"}
        }),
        json!({
            "type": "tool_call_response",
            "timestamp": "2025-01-01T00:00:03Z",
            "id": "tc_1",
            "content": "plain text result",
            "is_error": false
        }),
    ];

    let rendered = render_events(&events);
    match &rendered[1] {
        RenderedEvent::ToolCall {
            result, arguments, ..
        } => {
            assert_eq!(result.as_deref(), Some("plain text result"));
            assert!(arguments.contains("value"));
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

// RenderedEvent doesn't derive Debug, add a basic impl for panic messages.
impl std::fmt::Debug for RenderedEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TurnSeparator => write!(f, "TurnSeparator"),
            Self::UserMessage { .. } => write!(f, "UserMessage"),
            Self::AssistantMessage { .. } => write!(f, "AssistantMessage"),
            Self::Reasoning { .. } => write!(f, "Reasoning"),
            Self::Structured { .. } => write!(f, "Structured"),
            Self::ToolCall { name, .. } => write!(f, "ToolCall({name})"),
        }
    }
}
