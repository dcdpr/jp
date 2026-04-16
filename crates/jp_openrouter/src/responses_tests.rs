use test_log::test;

use super::*;

#[test]
fn test_simple_request_serialization() {
    let request = OpenResponsesRequest::new("gpt-4", "Hello, world!");

    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("gpt-4"));
    assert!(json.contains("Hello, world!"));
}

#[test]
fn test_request_with_tools() {
    let request = OpenResponsesRequest::new("gpt-4", "Search for Rust").with_tools(vec![
        Tool::WebSearchPreview {
            search_context_size: None,
            user_location: None,
        },
    ]);

    let json = serde_json::to_string_pretty(&request).unwrap();
    assert!(json.contains("web_search_preview"));
}

#[test]
fn test_function_tool() {
    let tool = Tool::Function {
        name: "get_weather".to_string(),
        description: Some("Get the current weather".to_string()),
        strict: Some(true),
        parameters: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "location": { "type": "string" }
            },
            "required": ["location"]
        })),
    };

    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains("get_weather"));
    assert!(json.contains("function"));
}

#[test]
fn test_provider_config() {
    let provider = Provider {
        allow_fallbacks: Some(true),
        data_collection: Some(DataCollection::Deny),
        sort: Some(ProviderSort::Price),
        ..Default::default()
    };

    let json = serde_json::to_string(&provider).unwrap();
    assert!(json.contains("allow_fallbacks"));
    assert!(json.contains("deny"));
    assert!(json.contains("price"));
}

#[test]
fn test_tool_choice_variants() {
    // Test mode variant
    let auto = ToolChoice::Auto;
    let json = serde_json::to_string(&auto).unwrap();
    assert_eq!(json, "\"auto\"");

    // Test function variant
    let func = ToolChoice::Function {
        name: "my_function".to_string(),
    };
    let json = serde_json::to_string(&func).unwrap();
    assert!(json.contains("my_function"));
    assert!(json.contains("function"));
}

#[test]
fn test_text_delta_event_deserialization() {
    let json = r#"{
            "type": "response.output_text.delta",
            "output_index": 0,
            "item_id": "item_123",
            "content_index": 0,
            "delta": "Hello",
            "logprobs": [],
            "sequence_number": 1
        }"#;

    let event: StreamEvent = serde_json::from_str(json).unwrap();
    assert!(matches!(event, StreamEvent::OutputTextDelta { .. }));
    assert_eq!(event.as_text_delta(), Some("Hello"));
    assert_eq!(event.sequence_number(), 1);
}

#[test]
fn test_response_completed_event() {
    let json = r#"{
            "type": "response.completed",
            "response": {
                "id": "resp_123",
                "object": "response",
                "created_at": 1234567890,
                "model": "gpt-4",
                "output": []
            },
            "sequence_number": 10
        }"#;

    let event: StreamEvent = serde_json::from_str(json).unwrap();
    assert!(event.is_terminal());
    assert!(!event.is_error());
    assert_eq!(event.event_type(), "response.completed");
}

#[test]
fn test_error_event() {
    let json = r#"{
            "type": "error",
            "code": "rate_limit_exceeded",
            "message": "Too many requests",
            "param": null,
            "sequence_number": 5
        }"#;

    let event: StreamEvent = serde_json::from_str(json).unwrap();
    assert!(event.is_error());
    assert_eq!(event.event_type(), "error");
}

#[test]
fn test_function_call_arguments_done() {
    let json = r#"{
            "type": "response.function_call_arguments.done",
            "item_id": "item_456",
            "output_index": 0,
            "name": "get_weather",
            "arguments": "{\"location\": \"London\"}",
            "sequence_number": 3
        }"#;

    let event: StreamEvent = serde_json::from_str(json).unwrap();
    match event {
        StreamEvent::FunctionCallArgumentsDone {
            name, arguments, ..
        } => {
            assert_eq!(name, "get_weather");
            assert!(arguments.contains("London"));
        }
        _ => panic!("Expected FunctionCallArgumentsDone"),
    }
}

#[test]
fn test_response_format_variants() {
    let text = ResponseFormat::Text;
    let json = serde_json::to_string(&text).unwrap();
    assert!(json.contains("\"type\":\"text\""));

    let json_obj = ResponseFormat::JsonObject;
    let json = serde_json::to_string(&json_obj).unwrap();
    assert!(json.contains("\"type\":\"json_object\""));

    let json_schema = ResponseFormat::JsonSchema {
        name: "person".to_string(),
        description: Some("A person".to_string()),
        schema: serde_json::json!({"type": "object"}),
        strict: Some(true),
    };
    let json = serde_json::to_string(&json_schema).unwrap();
    assert!(json.contains("\"type\":\"json_schema\""));
    assert!(json.contains("\"name\":\"person\""));
}

#[test]
fn test_keepalive_event() {
    let json = r#"{"type": "keepalive", "sequence_number": 12}"#;
    let event: StreamEvent = serde_json::from_str(json).unwrap();
    assert!(matches!(event, StreamEvent::Keepalive { .. }));
    assert_eq!(event.sequence_number(), 12);
    assert_eq!(event.event_type(), "keepalive");
}

#[test]
fn test_keepalive_event_without_sequence_number() {
    let json = r#"{"type": "keepalive"}"#;
    let event: StreamEvent = serde_json::from_str(json).unwrap();
    assert!(matches!(event, StreamEvent::Keepalive { .. }));
    assert_eq!(event.sequence_number(), 0);
}

#[test]
fn test_ping_event() {
    let json = r#"{"type": "ping", "sequence_number": 1}"#;
    let event: StreamEvent = serde_json::from_str(json).unwrap();
    assert!(matches!(event, StreamEvent::Ping { .. }));
    assert_eq!(event.event_type(), "ping");
}

#[test]
fn test_unknown_event_type() {
    let json = r#"{"type": "some_future_event", "data": "whatever"}"#;
    let event: StreamEvent = serde_json::from_str(json).unwrap();
    assert!(matches!(event, StreamEvent::Unknown));
    assert_eq!(event.event_type(), "unknown");
}

#[test]
fn test_plugin_variants() {
    let moderation = Plugin::Moderation;
    let json = serde_json::to_string(&moderation).unwrap();
    assert!(json.contains("\"id\":\"moderation\""));

    let web = Plugin::Web {
        max_results: Some(10),
        search_prompt: None,
        engine: Some(WebSearchEngine::Exa),
    };
    let json = serde_json::to_string(&web).unwrap();
    assert!(json.contains("\"id\":\"web\""));
    assert!(json.contains("\"max_results\":10"));
}
