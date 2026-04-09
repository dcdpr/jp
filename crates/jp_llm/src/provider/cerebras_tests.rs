use super::*;
use crate::provider::llamacpp::StreamChunk;

#[test]
fn parse_cerebras_content_chunk() {
    let json = r#"{
        "choices": [{
            "delta": { "content": "Hello!" },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello!"));
    assert!(chunk.choices[0].finish_reason.is_none());
}

#[test]
fn parse_cerebras_reasoning_field() {
    // Cerebras parsed format returns reasoning in the `reasoning` field, which
    // is deserialized into `reasoning_content` via serde alias.
    let json = r#"{
        "choices": [{
            "delta": {
                "reasoning": "The user just says hello.",
                "content": null
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert_eq!(
        delta.reasoning_content.as_deref(),
        Some("The user just says hello.")
    );
    assert!(delta.content.is_none());
}

#[test]
fn parse_cerebras_reasoning_content_field() {
    // The `reasoning_content` field name also works (DeepSeek-compatible).
    let json = r#"{
        "choices": [{
            "delta": {
                "reasoning_content": "step by step",
                "content": null
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert_eq!(delta.reasoning_content.as_deref(), Some("step by step"));
}

#[test]
fn parse_cerebras_finish_reason_stop() {
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
fn parse_cerebras_tool_call() {
    let json = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_xyz",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"foo.rs\"}"
                    }
                }]
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
    assert_eq!(tc.id.as_deref(), Some("call_xyz"));
    assert_eq!(
        tc.function.as_ref().unwrap().name.as_deref(),
        Some("read_file")
    );
}

#[test]
fn convert_tool_choice_values() {
    assert_eq!(convert_tool_choice(&ToolChoice::Auto), json!("auto"));
    assert_eq!(convert_tool_choice(&ToolChoice::None), json!("none"));
    assert_eq!(
        convert_tool_choice(&ToolChoice::Required),
        json!("required")
    );
    assert_eq!(
        convert_tool_choice(&ToolChoice::Function("my_fn".into())),
        json!({"type": "function", "function": {"name": "my_fn"}})
    );
}

#[test]
fn map_model_known() {
    let details = map_model("llama3.1-8b").unwrap();
    assert_eq!(details.display_name.as_deref(), Some("Llama 3.1 8B"));
    assert_eq!(details.context_window, Some(32_768));
    assert_eq!(details.max_output_tokens, Some(8_192));
    assert!(details.reasoning.unwrap().is_unsupported());
}

#[test]
fn map_model_gpt_oss_has_reasoning() {
    let details = map_model("gpt-oss-120b").unwrap();
    assert!(details.reasoning.unwrap().is_leveled());
}

#[test]
fn map_model_unknown_returns_empty() {
    let details = map_model("some-future-model").unwrap();
    assert!(details.display_name.is_none());
    assert!(details.context_window.is_none());
}

#[test]
fn transform_schema_moves_array_constraints_to_description() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "tags": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1,
                "maxItems": 5
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let tags = &result["properties"]["tags"];

    // Unsupported fields removed from schema.
    assert!(tags.get("minItems").is_none());
    assert!(tags.get("maxItems").is_none());

    // But preserved as a description hint.
    let desc = tags["description"].as_str().unwrap();
    assert!(desc.contains("minItems"), "desc = {desc}");
    assert!(desc.contains("maxItems"), "desc = {desc}");

    // Supported fields still present.
    assert_eq!(tags["type"], "array");
    assert_eq!(tags["items"]["type"], "string");
}

#[test]
fn transform_schema_moves_string_constraints_to_description() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "email": {
                "type": "string",
                "description": "An email address",
                "format": "email",
                "pattern": "^.+@.+$"
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let email = &result["properties"]["email"];

    assert!(email.get("format").is_none());
    assert!(email.get("pattern").is_none());

    // Hints appended to existing description.
    let desc = email["description"].as_str().unwrap();
    assert!(desc.starts_with("An email address"), "desc = {desc}");
    assert!(desc.contains("format"), "desc = {desc}");
    assert!(desc.contains("pattern"), "desc = {desc}");
}

#[test]
fn transform_schema_forces_strict_objects() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "nested": {
                "type": "object",
                "properties": {
                    "value": { "type": "integer" }
                }
            }
        },
        "required": ["name"]
    }))
    .unwrap();

    let result = transform_schema(schema);

    // Root object: additionalProperties false, all props required.
    assert_eq!(result["additionalProperties"], false);
    let required = result["required"].as_array().unwrap();
    assert!(required.contains(&json!("name")));
    assert!(required.contains(&json!("nested")));

    // Nested object: same treatment.
    let nested = &result["properties"]["nested"];
    assert_eq!(nested["additionalProperties"], false);
    let nested_req = nested["required"].as_array().unwrap();
    assert!(nested_req.contains(&json!("value")));
}

#[test]
fn transform_schema_preserves_number_constraints() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "age": {
                "type": "integer",
                "minimum": 0,
                "maximum": 150
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let age = &result["properties"]["age"];
    assert_eq!(age["minimum"], 0);
    assert_eq!(age["maximum"], 150);
    assert!(age.get("description").is_none());
}
