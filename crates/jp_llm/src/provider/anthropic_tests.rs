use indexmap::IndexMap;
use jp_config::model::parameters::{
    PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningEffort,
};
use jp_test::{Result, function_name};
use test_log::test;

use super::*;
use crate::test::{TestRequest, run_test};

const MAGIC_STRING: &str = "ANTHROPIC_MAGIC_STRING_TRIGGER_REDACTED_THINKING_46C9A13E193C177646C7398A98432ECCCE4C1253D5E2D82641AC0E52CC2876CB";

#[test(tokio::test)]
async fn test_redacted_thinking() -> Result {
    let requests = vec![
        TestRequest::chat(PROVIDER)
            .enable_reasoning()
            .chat_request(MAGIC_STRING),
        TestRequest::chat(PROVIDER)
            .chat_request("Do you have access to your redacted thinking content?"),
    ];

    run_test(PROVIDER, function_name!(), requests).await
}

#[test(tokio::test)]
async fn test_request_chaining() -> Result {
    let mut request = TestRequest::chat(PROVIDER)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Absolute(1024.into())),
                exclude: Some(false),
            },
        )))
        .chat_request("Give me a 2000 word explainer about Kirigami-inspired parachutes");

    if let Some(details) = request.as_model_details_mut() {
        details.max_output_tokens = Some(1152);
    }

    run_test(PROVIDER, function_name!(), Some(request)).await
}

/// Test that Opus 4.6 uses adaptive thinking mode with the effort parameter.
#[test(tokio::test)]
async fn test_opus_4_6_adaptive_thinking() -> Result {
    let mut request = TestRequest::chat(PROVIDER)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::High),
                exclude: Some(false),
            },
        )))
        .model("anthropic/claude-opus-4-6".parse().unwrap())
        .chat_request("What is 2 + 2?");

    // Configure model to use adaptive thinking (Opus 4.6 feature)
    if let Some(details) = request.as_model_details_mut() {
        details.reasoning = Some(ReasoningDetails::adaptive(true));
        details.features = vec!["adaptive-thinking"];
    }

    run_test(PROVIDER, function_name!(), Some(request)).await
}

/// Test Opus 4.6 with `Max` effort level (only supported on Opus 4.6).
#[test(tokio::test)]
async fn test_opus_4_6_max_effort() -> Result {
    let mut request = TestRequest::chat(PROVIDER)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Max),
                exclude: Some(false),
            },
        )))
        .model("anthropic/claude-opus-4-6".parse().unwrap())
        .chat_request("What is 2 + 2?");

    // Configure model to use adaptive thinking with max effort support (Opus 4.6 feature)
    if let Some(details) = request.as_model_details_mut() {
        details.reasoning = Some(ReasoningDetails::adaptive(true));
        details.features = vec!["adaptive-thinking"];
    }

    run_test(PROVIDER, function_name!(), Some(request)).await
}

/// Unit test: Verify Opus 4.6 generates adaptive thinking request.
#[test]
fn test_opus_4_6_request_uses_adaptive_thinking() {
    use jp_conversation::{ConversationStream, thread::Thread};

    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: Some("Claude Opus 4.6".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(true)),
        knowledge_cutoff: None,
        deprecated: None,
        features: vec!["adaptive-thinking"],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_chat_request("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, is_structured) = create_request(&model, query, true, &beta).unwrap();
    assert!(!is_structured);

    // Verify adaptive thinking is used
    assert_eq!(request.thinking, Some(types::ExtendedThinking::Adaptive));

    // Verify output_config has effort set (defaults to High)
    assert!(request.output_config.is_some());
    let output_config = request.output_config.unwrap();
    assert_eq!(output_config.effort, Some(Effort::High));
    assert_eq!(output_config.format, None);
}

/// Unit test: Verify Max effort maps to `Effort::Max` for Opus 4.6.
#[test]
fn test_opus_4_6_max_effort_mapping() {
    use jp_conversation::{ConversationStream, thread::Thread};

    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: Some("Claude Opus 4.6".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(true)), // supports max
        knowledge_cutoff: None,
        deprecated: None,
        features: vec!["adaptive-thinking"],
    };

    let mut events = ConversationStream::new_test().with_chat_request("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Custom(
        PartialCustomReasoningConfig {
            effort: Some(ReasoningEffort::Max),
            exclude: Some(false),
        },
    ));
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _) = create_request(&model, query, true, &beta).unwrap();

    // Verify Max effort is used
    assert!(request.output_config.is_some());
    let output_config = request.output_config.unwrap();
    assert_eq!(output_config.effort, Some(Effort::Max));
}

/// Unit test: Verify budget-based model (Opus 4.5) still uses Enabled thinking.
#[test]
fn test_opus_4_5_uses_budgetted_thinking() {
    use jp_conversation::{ConversationStream, thread::Thread};

    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-5").try_into().unwrap(),
        display_name: Some("Claude Opus 4.5".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        features: vec!["interleaved-thinking"],
    };

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_chat_request("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, _) = create_request(&model, query, true, &beta).unwrap();

    // Verify budget-based thinking is used (not adaptive)
    assert!(matches!(
        request.thinking,
        Some(types::ExtendedThinking::Enabled { .. })
    ));

    // Verify output_config is NOT set for budget-based models
    assert!(request.output_config.is_none());
}

/// Verify structured output sets `output_config.format` when the last event
/// is a `ChatRequest` with a schema.
#[test]
fn test_structured_output_sets_format() {
    use jp_conversation::{ConversationStream, event::ChatRequest, thread::Thread};
    use serde_json::json;

    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: Some("Claude Sonnet 4.5".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        features: vec!["structured-outputs"],
    };

    let schema = serde_json::Map::from_iter([
        ("type".into(), json!("object")),
        ("properties".into(), json!({"name": {"type": "string"}})),
    ]);

    let events = ConversationStream::new_test().with_chat_request(ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(schema),
    });

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, is_structured) = create_request(&model, query, true, &beta).unwrap();

    assert!(is_structured);
    assert!(request.output_config.is_some());
    let output_config = request.output_config.unwrap();
    // No adaptive thinking, so effort should be None.
    assert_eq!(output_config.effort, None);
    // transform_schema adds additionalProperties: false for objects.
    let expected_schema = serde_json::Map::from_iter([
        ("type".into(), json!("object")),
        ("properties".into(), json!({"name": {"type": "string"}})),
        ("additionalProperties".into(), json!(false)),
    ]);
    assert_eq!(
        output_config.format,
        Some(JsonOutputFormat::JsonSchema {
            schema: expected_schema
        })
    );
}

/// When the last event is NOT a `ChatRequest` (e.g. a `ChatResponse`), no
/// structured output should be configured even if a prior `ChatRequest` had a
/// schema.
#[test]
fn test_schema_ignored_when_last_event_is_not_chat_request() {
    use jp_conversation::{
        ConversationStream,
        event::{ChatRequest, ChatResponse},
        thread::Thread,
    };
    use serde_json::json;

    let model = ModelDetails {
        id: (PROVIDER, "claude-sonnet-4-5").try_into().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: None,
        knowledge_cutoff: None,
        deprecated: None,
        features: vec![],
    };

    let mut events = ConversationStream::new_test();
    // First turn: structured request
    events.add_chat_request(ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(serde_json::Map::from_iter([(
            "type".into(),
            json!("object"),
        )])),
    });
    // Then a response (now the last event is not a ChatRequest)
    events.add_chat_response(ChatResponse::structured(json!({"name": "Alice"})));
    // Follow-up without schema
    events.add_chat_request(ChatRequest {
        content: "Explain what you found".into(),
        schema: None,
    });

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (_, is_structured) = create_request(&model, query, true, &beta).unwrap();
    assert!(!is_structured);
}

/// Adaptive thinking + structured output should coexist on `OutputConfig`.
#[test]
fn test_adaptive_thinking_with_structured_output() {
    use jp_conversation::{ConversationStream, event::ChatRequest, thread::Thread};
    use serde_json::json;

    let model = ModelDetails {
        id: (PROVIDER, "claude-opus-4-6").try_into().unwrap(),
        display_name: Some("Claude Opus 4.6".to_string()),
        context_window: Some(200_000),
        max_output_tokens: Some(128_000),
        reasoning: Some(ReasoningDetails::adaptive(true)),
        knowledge_cutoff: None,
        deprecated: None,
        features: vec!["adaptive-thinking", "structured-outputs"],
    };

    let schema = serde_json::Map::from_iter([("type".into(), json!("object"))]);

    let events = ConversationStream::new_test().with_chat_request(ChatRequest {
        content: "Extract data".into(),
        schema: Some(schema),
    });

    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let beta = BetaFeatures(vec![]);
    let (request, is_structured) = create_request(&model, query, true, &beta).unwrap();

    assert!(is_structured);
    assert_eq!(request.thinking, Some(types::ExtendedThinking::Adaptive));

    let output_config = request.output_config.unwrap();
    // Both effort and format should be present.
    assert_eq!(output_config.effort, Some(Effort::High));
    let expected_schema = serde_json::Map::from_iter([
        ("type".into(), json!("object")),
        ("additionalProperties".into(), json!(false)),
    ]);
    assert_eq!(
        output_config.format,
        Some(JsonOutputFormat::JsonSchema {
            schema: expected_schema
        })
    );
}

#[test]
fn test_find_merge_point_edge_cases() {
    struct TestCase {
        left: &'static str,
        right: &'static str,
        expected: &'static str,
        max_search: usize,
    }

    let cases = IndexMap::from([
        ("no overlap", TestCase {
            left: "Hello",
            right: " world",
            expected: "Hello world",
            max_search: 500,
        }),
        ("single word overlap", TestCase {
            left: "The quick brown",
            right: "brown fox",
            expected: "The quick brown fox",
            max_search: 500,
        }),
        ("minimal overlap (5 chars)", TestCase {
            expected: "abcdefghij",
            left: "abcdefgh",
            right: "defghij",
            max_search: 500,
        }),
        (
            "below minimum overlap (4 chars) - should not merge",
            TestCase {
                left: "abcd",
                right: "abcd",
                expected: "abcdabcd",
                max_search: 500,
            },
        ),
        ("complete overlap", TestCase {
            left: "Hello world",
            right: "world",
            expected: "Hello world",
            max_search: 500,
        }),
        ("overlap with punctuation", TestCase {
            left: "Hello, how are",
            right: "how are you?",
            expected: "Hello, how are you?",
            max_search: 500,
        }),
        ("overlap with whitespace", TestCase {
            left: "Hello     ",
            right: "     world",
            expected: "Hello     world",
            max_search: 500,
        }),
        ("unicode overlap", TestCase {
            left: "Hello 世界",
            right: "世界 friend",
            expected: "Hello 世界 friend",
            max_search: 500,
        }),
        ("long overlap", TestCase {
            left: "The quick brown fox jumps",
            right: "fox jumps over the lazy dog",
            expected: "The quick brown fox jumpsfox jumps over the lazy dog",
            max_search: 8,
        }),
        ("empty right", TestCase {
            left: "Hello",
            right: "",
            expected: "Hello",
            max_search: 500,
        }),
    ]);

    for (
        name,
        TestCase {
            left,
            right,
            expected,
            max_search,
        },
    ) in cases
    {
        let pos = find_merge_point(left, right, max_search);
        let result = format!("{left}{}", &right[pos..]);
        assert_eq!(result, expected, "Failed test case: {name}");
    }
}

mod transform_schema {
    use serde_json::{Map, Value, json};

    use super::transform_schema;

    #[expect(clippy::needless_pass_by_value)]
    fn schema(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn object_forces_additional_properties_false() {
        let input = schema(json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }));

        let out = transform_schema(input);

        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["name"]));
        assert_eq!(out["properties"]["name"]["type"], "string");
    }

    #[test]
    fn object_drops_existing_additional_properties() {
        let input = schema(json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true
        }));

        let out = transform_schema(input);
        assert_eq!(out["additionalProperties"], json!(false));
    }

    #[test]
    fn array_keeps_min_items_0_and_1() {
        for n in [0, 1] {
            let input = schema(json!({
                "type": "array",
                "items": { "type": "string" },
                "minItems": n
            }));

            let out = transform_schema(input);
            assert_eq!(out["minItems"], json!(n), "minItems {n} should be kept");
        }
    }

    #[test]
    fn array_moves_large_min_items_to_description() {
        let input = schema(json!({
            "type": "array",
            "items": { "type": "string" },
            "minItems": 3
        }));

        let out = transform_schema(input);
        assert!(out.get("minItems").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.contains("minItems"),
            "description should mention minItems: {desc}"
        );
    }

    #[test]
    fn array_moves_max_items_to_description() {
        let input = schema(json!({
            "type": "array",
            "items": { "type": "string" },
            "maxItems": 5
        }));

        let out = transform_schema(input);
        assert!(out.get("maxItems").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.contains("maxItems"),
            "description should mention maxItems: {desc}"
        );
    }

    #[test]
    fn string_keeps_supported_format() {
        let input = schema(json!({
            "type": "string",
            "format": "date-time"
        }));

        let out = transform_schema(input);
        assert_eq!(out["format"], "date-time");
        assert!(out.get("description").is_none());
    }

    #[test]
    fn string_moves_unsupported_format_to_description() {
        let input = schema(json!({
            "type": "string",
            "format": "phone-number"
        }));

        let out = transform_schema(input);
        assert!(out.get("format").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.contains("phone-number"),
            "description should contain the format: {desc}"
        );
    }

    #[test]
    fn numeric_constraints_moved_to_description() {
        let input = schema(json!({
            "type": "integer",
            "minimum": 1,
            "maximum": 10,
            "description": "A number"
        }));

        let out = transform_schema(input);
        assert!(out.get("minimum").is_none());
        assert!(out.get("maximum").is_none());
        let desc = out["description"].as_str().unwrap();
        assert!(
            desc.starts_with("A number"),
            "should preserve original description"
        );
        assert!(desc.contains("minimum"), "should contain minimum: {desc}");
        assert!(desc.contains("maximum"), "should contain maximum: {desc}");
    }

    #[test]
    fn ref_passes_through() {
        let input = schema(json!({
            "$ref": "#/$defs/Address"
        }));

        let out = transform_schema(input);
        assert_eq!(out["$ref"], "#/$defs/Address");
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn defs_recursively_transformed() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "addr": { "$ref": "#/$defs/Address" }
            },
            "$defs": {
                "Address": {
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "additionalProperties": true
                }
            }
        }));

        let out = transform_schema(input);
        let addr_def = out["$defs"]["Address"].as_object().unwrap();
        assert_eq!(addr_def["additionalProperties"], json!(false));
    }

    #[test]
    fn one_of_converted_to_any_of() {
        let input = schema(json!({
            "oneOf": [
                { "type": "string" },
                { "type": "integer" }
            ]
        }));

        let out = transform_schema(input);
        assert!(out.get("oneOf").is_none());
        let any_of = out["anyOf"].as_array().unwrap();
        assert_eq!(any_of.len(), 2);
        assert_eq!(any_of[0]["type"], "string");
        assert_eq!(any_of[1]["type"], "integer");
    }

    #[test]
    fn nested_properties_recursively_transformed() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": { "id": { "type": "integer", "minimum": 0 } },
                        "additionalProperties": true
                    },
                    "maxItems": 10
                }
            }
        }));

        let out = transform_schema(input);

        // Top-level object
        assert_eq!(out["additionalProperties"], json!(false));

        // The array property
        let items_prop = out["properties"]["items"].as_object().unwrap();
        assert!(items_prop.get("maxItems").is_none());

        // The nested object inside the array
        let nested = items_prop["items"].as_object().unwrap();
        assert_eq!(nested["additionalProperties"], json!(false));

        // The integer property's minimum should be in description
        let id_prop = nested["properties"]["id"].as_object().unwrap();
        assert!(id_prop.get("minimum").is_none());
        let desc = id_prop["description"].as_str().unwrap();
        assert!(
            desc.contains("minimum"),
            "nested constraint in description: {desc}"
        );
    }

    /// Mirrors the example from Anthropic's Python SDK docstring.
    #[test]
    fn sdk_docstring_example() {
        let input = schema(json!({
            "type": "integer",
            "minimum": 1,
            "maximum": 10,
            "description": "A number"
        }));

        let out = transform_schema(input);
        assert_eq!(out["type"], "integer");
        let desc = out["description"].as_str().unwrap();
        assert!(desc.starts_with("A number"));
        assert!(desc.contains("minimum: 1"));
        assert!(desc.contains("maximum: 10"));
    }

    /// The `title_schema` used by the title generator should survive
    /// transformation.
    #[test]
    fn title_schema_transforms_cleanly() {
        let input = crate::title::title_schema(3);
        let out = transform_schema(input);

        assert_eq!(out["type"], "object");
        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["titles"]));

        let titles = out["properties"]["titles"].as_object().unwrap();
        assert_eq!(titles["type"], "array");
        // minItems 1 is kept but > 1 is moved to description
        assert!(titles.get("minItems").is_none());
        assert!(titles.get("maxItems").is_none());
        let desc = titles["description"].as_str().unwrap();
        assert!(
            desc.contains("minItems"),
            "should contain minItems hint: {desc}"
        );
        assert!(
            desc.contains("maxItems"),
            "should contain maxItems hint: {desc}"
        );
    }
}
