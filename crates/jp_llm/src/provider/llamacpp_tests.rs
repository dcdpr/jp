use super::*;

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
    events.push(ConversationEvent::now(ToolCallRequest {
        id: "call_1".into(),
        name: "tool_a".into(),
        arguments: serde_json::Map::new(),
    }));
    events.push(ConversationEvent::now(ToolCallRequest {
        id: "call_2".into(),
        name: "tool_b".into(),
        arguments: serde_json::Map::new(),
    }));

    let messages = convert_events(events);

    // Should be merged into a single assistant message with 2 tool_calls.
    assert_eq!(messages.len(), 1);
    let tool_calls = messages[0]["tool_calls"].as_array().unwrap();
    assert_eq!(tool_calls.len(), 2);
    assert_eq!(tool_calls[0]["function"]["name"], "tool_a");
    assert_eq!(tool_calls[1]["function"]["name"], "tool_b");
}

#[test]
fn convert_events_wraps_reasoning_in_think_tags() {
    let mut events = ConversationStream::new_test();
    events.push(ConversationEvent::now(ChatResponse::reasoning(
        "step 1: think hard",
    )));

    let messages = convert_events(events);

    assert_eq!(messages.len(), 1);
    let content = messages[0]["content"].as_str().unwrap();
    assert!(content.starts_with("<think>"));
    assert!(content.contains("step 1: think hard"));
    assert!(content.ends_with("</think>"));
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
