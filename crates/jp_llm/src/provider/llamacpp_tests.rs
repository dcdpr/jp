use jp_config::assistant::tool_choice::ToolChoice;

use super::*;
use crate::provider::openai_compat::StreamChunk;

fn qwen_model() -> LlamacppModel {
    serde_json::from_value(serde_json::json!({
        "id": "unsloth/Qwen3.5-9B-GGUF",
        "meta": {"n_ctx_train": 262_144},
    }))
    .unwrap()
}

/// The served context window wins over the trained one.
/// A server launched with a smaller `--ctx-size` than the model was trained for
/// is the normal local setup, and the served value is what a request is
/// actually bounded by.
#[test]
fn map_model_prefers_served_context_length() {
    let details = map_model(&qwen_model(), Some(8_192)).unwrap();

    assert_eq!(details.context_window, Some(8_192));
    // The vendor prefix is stripped from the id.
    assert_eq!(details.id.name.as_ref(), "Qwen3.5-9B-GGUF");
}

/// Without `/props` the trained length is the only figure available, so it is
/// used even though it can over-report the served window.
#[test]
fn map_model_falls_back_to_trained_context_length() {
    let details = map_model(&qwen_model(), None).unwrap();

    assert_eq!(details.context_window, Some(262_144));
}

/// An older build reporting neither `/props` nor metadata leaves the context
/// window unknown rather than asserting a value.
#[test]
fn map_model_without_meta_leaves_context_unknown() {
    let model: LlamacppModel =
        serde_json::from_value(serde_json::json!({"id": "local-model"})).unwrap();

    let details = map_model(&model, None).unwrap();

    assert_eq!(details.context_window, None);
    assert_eq!(details.reasoning, None);
}

/// Build a query whose only config is an explicit reasoning setting.
fn reasoning_query(
    reasoning: Option<jp_config::model::parameters::PartialReasoningConfig>,
) -> ChatQuery {
    let mut events = jp_conversation::ConversationStream::new_test().with_turn("test");

    if let Some(reasoning) = reasoning {
        let mut delta = jp_config::PartialAppConfig::empty();
        delta.assistant.model.parameters.reasoning = Some(reasoning);
        events.add_config_delta(delta);
    }

    ChatQuery {
        thread: jp_conversation::thread::Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    }
}

fn request_body(
    reasoning: Option<jp_config::model::parameters::PartialReasoningConfig>,
) -> serde_json::Value {
    let details = ModelDetails::empty((PROVIDER, "Qwen3.5-9B-GGUF").try_into().unwrap());
    let (request, _) = create_request(&details, reasoning_query(reasoning)).unwrap();

    request
}

/// `reasoning_format` tells the server how to parse a thinking block, not
/// whether the model produces one.
/// Asking for `none` leaves llama.cpp unable to place a grammar after the
/// block, so a structured-output request is rejected with `Failed to initialize
/// samplers` for any thinking-capable model.
#[test]
fn create_request_always_parses_reasoning() {
    use jp_config::model::parameters::PartialReasoningConfig;

    for reasoning in [
        None,
        Some(PartialReasoningConfig::Off),
        Some(PartialReasoningConfig::Auto),
    ] {
        assert_eq!(
            request_body(reasoning)["reasoning_format"],
            serde_json::json!("deepseek")
        );
    }
}

/// Whether the model thinks at all is the chat template's decision, driven by
/// `enable_thinking`.
/// A template that does not read the kwarg ignores it, and any reasoning it
/// emits then arrives parsed rather than inline in the answer.
#[test]
fn create_request_asks_the_template_to_skip_thinking_when_reasoning_is_off() {
    use jp_config::model::parameters::PartialReasoningConfig;

    assert_eq!(
        request_body(Some(PartialReasoningConfig::Off))["chat_template_kwargs"],
        serde_json::json!({ "enable_thinking": false })
    );
    assert_eq!(
        request_body(Some(PartialReasoningConfig::Auto))["chat_template_kwargs"],
        serde_json::json!({ "enable_thinking": true })
    );
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
