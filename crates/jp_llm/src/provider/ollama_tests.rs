use serde_json::json;

use super::*;
use crate::tool::ToolDocs;

/// Ollama drops `$ref` while decoding a tool's parameters, so a referenced type
/// has to arrive expanded or the model sees a property with no type.
#[test]
fn tool_references_are_expanded() {
    let tools = vec![ToolDefinition {
        name: "crate_search_items".to_owned(),
        docs: ToolDocs::default(),
        parameters: json!({
            "type": "object",
            "properties": {
                "kinds": { "type": "array", "items": { "$ref": "#/$defs/EntryType" } }
            },
            "required": ["kinds"],
            "$defs": { "EntryType": { "type": "string", "enum": ["Enum", "Method"] } }
        }),
    }];

    let converted = convert_tools(tools).expect("tools convert");
    let parameters = serde_json::to_value(&converted[0].function.parameters).expect("serializes");

    assert_eq!(
        parameters,
        json!({
            "type": "object",
            "properties": {
                "kinds": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["Enum", "Method"] }
                }
            },
            "required": ["kinds"]
        })
    );
}

/// The whole document is sent, not just its properties: Ollama reads `type` and
/// `required` from the same object.
#[test]
fn tool_parameters_keep_the_schema_document() {
    let tools = vec![ToolDefinition {
        name: "read_file".to_owned(),
        docs: ToolDocs::default(),
        parameters: json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    }];

    let converted = convert_tools(tools).expect("tools convert");
    let parameters = serde_json::to_value(&converted[0].function.parameters).expect("serializes");

    assert_eq!(parameters["type"], json!("object"));
    assert_eq!(parameters["required"], json!(["path"]));
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

fn think_flag(
    details: &ModelDetails,
    reasoning: Option<jp_config::model::parameters::PartialReasoningConfig>,
) -> bool {
    let (request, _) = create_request(details, reasoning_query(reasoning)).unwrap();
    serde_json::to_value(request).unwrap()["think"]
        .as_bool()
        .expect("request carries an explicit think flag")
}

/// A model advertising no `thinking` capability must not be asked to think,
/// even when the caller enables reasoning: Ollama rejects the request outright.
#[test]
fn create_request_does_not_ask_unsupported_model_to_think() {
    let mut details = ModelDetails::empty((PROVIDER, "llama3:latest").try_into().unwrap());
    details.reasoning = Some(ReasoningDetails::unsupported());

    assert!(!think_flag(
        &details,
        Some(jp_config::model::parameters::PartialReasoningConfig::Auto)
    ));
    assert!(!think_flag(
        &details,
        Some(
            jp_config::model::parameters::PartialReasoningConfig::Custom(
                jp_config::model::parameters::PartialCustomReasoningConfig {
                    effort: Some(jp_config::model::parameters::ReasoningEffort::High),
                    exclude: Some(false),
                },
            )
        )
    ));
}

/// A model whose support is unknown still honours an explicit request, leaving
/// the provider to reject it if the model genuinely cannot think.
#[test]
fn create_request_asks_unknown_model_to_think_when_requested() {
    let details = ModelDetails::empty((PROVIDER, "future-model-99").try_into().unwrap());
    assert_eq!(details.reasoning, None, "fixture must be unknown");

    assert!(think_flag(
        &details,
        Some(jp_config::model::parameters::PartialReasoningConfig::Auto)
    ));
}

/// Reasoning left unconfigured, or explicitly off, never asks the model to
/// think.
#[test]
fn create_request_does_not_think_when_unconfigured_or_off() {
    let details = ModelDetails::empty((PROVIDER, "future-model-99").try_into().unwrap());

    assert!(!think_flag(&details, None));
    assert!(!think_flag(
        &details,
        Some(jp_config::model::parameters::PartialReasoningConfig::Off)
    ));
}

/// Ollama reports the trained context length per model, so it is used rather
/// than left unknown.
/// A model advertising `thinking` keeps its reasoning support unknown, since no
/// effort ladder is reported to derive one from.
#[test]
fn map_model_uses_reported_context_and_capabilities() {
    let model: LocalModel = serde_json::from_value(serde_json::json!({
        "name": "qwen3.5:9b",
        "modified_at": "2026-03-06T03:05:33.981620282+01:00",
        "size": 6_594_474_711_u64,
        "details": {"family": "qwen35", "context_length": 262_144},
        "capabilities": ["vision", "completion", "tools", "thinking"],
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.context_window, Some(262_144));
    // Reports thinking, but no ladder, so support stays unknown.
    assert_eq!(details.reasoning, None);
    // `/api/tags` reports no generation ceiling.
    assert_eq!(details.max_output_tokens, None);
}

/// A model whose reported capabilities omit `thinking` does not reason.
#[test]
fn map_model_without_thinking_capability_is_unsupported() {
    let model: LocalModel = serde_json::from_value(serde_json::json!({
        "name": "llama3:latest",
        "modified_at": "2025-09-16T21:31:42.798881306+02:00",
        "size": 4_661_224_676_u64,
        "details": {"family": "llama", "context_length": 8_192},
        "capabilities": ["completion"],
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.reasoning, Some(ReasoningDetails::unsupported()));
    assert_eq!(details.context_window, Some(8_192));
}

/// An Ollama version that reports no capabilities leaves support unknown,
/// rather than an empty list being read as "this model does not reason".
#[test]
fn map_model_without_reported_capabilities_stays_unknown() {
    let model: LocalModel = serde_json::from_value(serde_json::json!({
        "name": "legacy:7b",
        "modified_at": "",
        "size": 0,
    }))
    .unwrap();

    let details = map_model(model).unwrap();

    assert_eq!(details.reasoning, None);
    assert_eq!(details.context_window, None);
}
