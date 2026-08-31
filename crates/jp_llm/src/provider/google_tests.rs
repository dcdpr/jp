use jp_config::model::parameters::{
    PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningEffort,
};
use jp_conversation::event::ChatRequest;
use jp_test::function_name;
use test_log::test;

use super::*;
use crate::test::{TestRequest, run_test};

// TODO: Test specific conditions as detailed in
// <https://ai.google.dev/gemini-api/docs/thought-signatures>:
//
// - parallel function calls
// - dummy thought signatures
// - multi-turn conversations
#[test(tokio::test)]
async fn test_gemini_3_reasoning() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let request = TestRequest::chat(PROVIDER)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Low),
                exclude: Some(false),
            },
        )))
        .model("google/gemini-3.1-pro-preview".parse().unwrap())
        .event(ChatRequest::from("Test message"));

    run_test(PROVIDER, function_name!(), Some(request)).await
}

/// Regression: when reasoning support is unknown (a model newer than this
/// binary) and reasoning is configured, the request must still ask for
/// thoughts.
/// Sending no thinking config lets Gemini think while withholding the thoughts,
/// billing reasoning tokens that never reach the transcript.
#[test]
fn test_unknown_model_requests_thoughts() {
    let model = ModelDetails::empty((PROVIDER, "gemini-future-99").try_into().unwrap());
    assert_eq!(model.reasoning, None, "fixture must have unknown reasoning");

    let mut events = jp_conversation::ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Auto);
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: jp_conversation::thread::Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let (request, _) = create_request(&model, query).unwrap();

    let thinking = request
        .generation_config
        .and_then(|c| c.thinking_config)
        .expect("thinking config must be sent for an unknown model");
    assert!(
        thinking.include_thoughts,
        "thoughts must be requested so reasoning reaches the transcript"
    );
}

/// End-to-end: the API's `thinking` flag overrides the table, so a model the
/// API reports as not thinking is never configured for it.
///
/// The fixture must be a model the table *does* carry a ladder for, otherwise
/// the catch-all supplies `None` and the override is never exercised.
#[test]
fn test_map_model_thinking_flag_overrides_table() {
    let model = types::Model {
        base_model_id: "gemini-2.5-flash".to_owned(),
        display_name: "Gemini 2.5 Flash".to_owned(),
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        thinking: false,
        ..Default::default()
    };

    // The table entry this overrides.
    assert!(
        map_model(types::Model {
            base_model_id: "gemini-2.5-flash".to_owned(),
            thinking: true,
            ..Default::default()
        })
        .reasoning
        .is_some_and(|r| r.is_budgetted()),
        "fixture must target a model the table gives a ladder"
    );

    let details = map_model(model);

    assert_eq!(details.reasoning, Some(ReasoningDetails::unsupported()));
    assert_eq!(details.context_window, Some(1_048_576));
    assert_eq!(details.max_output_tokens, Some(65_536));
}

/// A model whose ladder is unknown still honours the caller's effort.
/// Dropping it would make `-r high` silently do nothing on any thinking model
/// newer than this binary, which is most of Gemini's current catalog.
#[test]
fn test_unknown_thinking_level_honours_requested_effort() {
    assert_eq!(
        effort_to_thinking_level(ReasoningEffort::High, None),
        Some(types::ThinkingLevel::High)
    );
    assert_eq!(
        effort_to_thinking_level(ReasoningEffort::Low, None),
        Some(types::ThinkingLevel::Low)
    );
    assert_eq!(
        effort_to_thinking_level(ReasoningEffort::Medium, None),
        Some(types::ThinkingLevel::Medium)
    );

    // Efforts above `high` clamp down rather than being dropped.
    assert_eq!(
        effort_to_thinking_level(ReasoningEffort::Max, None),
        Some(types::ThinkingLevel::High)
    );

    // `auto` leaves the choice to the model.
    assert_eq!(effort_to_thinking_level(ReasoningEffort::Auto, None), None);
}

/// An explicit `off` on a model whose support is unknown sends the documented
/// disable rather than nothing.
///
/// Sending nothing takes a provider default that, on a modern model, thinks and
/// bills for reasoning the caller turned off.
/// A custom `base_url` may also serve a model that accepts the disable, so the
/// endpoint is the judge.
#[test]
fn test_off_on_unknown_model_attempts_disable() {
    let model = ModelDetails::empty((PROVIDER, "gemini-future-99").try_into().unwrap());
    assert_eq!(model.reasoning, None, "fixture must be unknown");

    let mut events = jp_conversation::ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Off);
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: jp_conversation::thread::Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let (request, _) = create_request(&model, query).unwrap();

    let thinking = request
        .generation_config
        .and_then(|c| c.thinking_config)
        .expect("an explicit off must not be silently dropped");

    assert!(!thinking.include_thoughts);
    assert_eq!(thinking.thinking_budget, Some(0));
}

/// A leveled model that cannot turn reasoning off must not be sent
/// `thinking_budget: 0` when reasoning is off: that both disables a model which
/// rejects being disabled and uses the budget form for a level-based model.
/// Its lowest supported level is requested with the thoughts withheld instead.
#[test]
fn test_off_on_always_on_leveled_model_uses_lowest_level() {
    let mut model = ModelDetails::empty((PROVIDER, "gemini-3.1-pro-preview").try_into().unwrap());
    model.max_output_tokens = Some(65_536);
    // Mirrors the table entry: low and high only, reasoning always on.
    model.reasoning =
        Some(ReasoningDetails::leveled(false, true, false, true, false, false).always_on());

    let mut events = jp_conversation::ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Off);
    events.add_config_delta(delta);

    let query = ChatQuery {
        thread: jp_conversation::thread::Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let (request, _) = create_request(&model, query).unwrap();

    let thinking = request
        .generation_config
        .and_then(|c| c.thinking_config)
        .expect("an always-on model still gets a thinking config");

    assert!(!thinking.include_thoughts, "thoughts are withheld");
    assert_eq!(
        thinking.thinking_budget, None,
        "a level-based model takes no token budget"
    );
    assert_eq!(thinking.thinking_level, Some(types::ThinkingLevel::Low));
}

/// A model the API reports as not thinking is recorded as not reasoning, even
/// when the built-in table claims a ladder for it.
/// Sending a thinking budget to such a model configures a mode it does not
/// have.
#[test]
fn test_apply_thinking_support_overrides_table() {
    let table = Some(ReasoningDetails::budgetted(0, Some(24_576)));

    assert_eq!(
        apply_thinking_support(table, false),
        Some(ReasoningDetails::unsupported())
    );
}

/// A thinking model keeps whatever the table says, including "unknown", since
/// the API reports no effort levels to derive a ladder from.
#[test]
fn test_apply_thinking_support_keeps_table_when_thinking() {
    let table = Some(ReasoningDetails::leveled(
        false, true, true, true, false, false,
    ));

    assert_eq!(apply_thinking_support(table, true), table);
    assert_eq!(apply_thinking_support(None, true), None);
}

/// Records a live request for a Gemini model absent from the built-in table,
/// with an explicit effort.
///
/// Validates the inference the unit tests cannot: that a model newer than this
/// binary accepts a `thinking_level`, rather than the token-budget form 2.x
/// models take.
/// Most of Gemini's current catalog reaches this path.
#[test(tokio::test)]
async fn test_unknown_model_inferred_thinking_level()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let id: ModelIdConfig = "google/gemini-3.5-flash".parse().unwrap();

    // Mirrors what `map_model` derives: limits from the API, and no reasoning
    // ladder because the model is absent from the table.
    let mut details = ModelDetails::empty(id.clone());
    details.context_window = Some(1_048_576);
    details.max_output_tokens = Some(65_536);
    assert_eq!(details.reasoning, None, "fixture must be unknown");

    let request = TestRequest::chat(PROVIDER)
        .model(id)
        .model_details(details)
        .reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::High),
                exclude: Some(false),
            },
        )))
        .event(ChatRequest::from("What is 2 + 2?"));

    run_test(PROVIDER, function_name!(), Some(request)).await
}

mod thought_signature_recovery {
    use gemini_client_rs::types;

    use crate::{
        error::StreamError,
        event::{EventMatcher, PatchAction},
        provider::google::{
            THOUGHT_SIGNATURE_DUMMY_VALUE, THOUGHT_SIGNATURE_KEY, build_thought_signature_patch,
            is_corrupted_thought_signature,
        },
    };

    fn text_part(text: &str) -> types::ContentPart {
        types::ContentPart {
            data: types::ContentData::Text(text.to_owned()),
            thought: false,
            thought_signature: None,
            metadata: None,
        }
    }

    fn thought_part(text: &str, sig: &str) -> types::ContentPart {
        types::ContentPart {
            data: types::ContentData::Text(text.to_owned()),
            thought: true,
            thought_signature: Some(sig.to_owned()),
            metadata: None,
        }
    }

    fn function_call_part(name: &str, sig: &str) -> types::ContentPart {
        types::ContentPart {
            data: types::ContentData::FunctionCall(types::FunctionCall {
                name: name.to_owned(),
                id: None,
                arguments: serde_json::Value::Null,
            }),
            thought: false,
            thought_signature: Some(sig.to_owned()),
            metadata: None,
        }
    }

    fn function_response_part(name: &str) -> types::ContentPart {
        types::ContentPart {
            data: types::ContentData::FunctionResponse(types::FunctionResponse {
                name: name.to_owned(),
                id: None,
                response: types::FunctionResponsePayload {
                    content: serde_json::Value::String("ok".to_owned()),
                },
            }),
            thought: false,
            thought_signature: None,
            metadata: None,
        }
    }

    fn content(role: types::Role, parts: Vec<types::ContentPart>) -> types::Content {
        types::Content {
            role: Some(role),
            parts,
        }
    }

    fn request(contents: Vec<types::Content>) -> types::GenerateContentRequest {
        types::GenerateContentRequest {
            system_instruction: None,
            contents,
            tools: vec![],
            tool_config: None,
            generation_config: None,
        }
    }

    #[test]
    fn detects_corrupted_signature_error() {
        let err = StreamError::other(
            "API Error: \
             {\"status\":400,\"message\":{\"error\":{\"code\":400,\"message\":\"Corrupted thought \
             signature.\",\"status\":\"INVALID_ARGUMENT\"}},\"context\":{\"cause\":\"Invalid \
             status code\"}}",
        );
        assert!(is_corrupted_thought_signature(&err));
    }

    #[test]
    fn ignores_unrelated_errors() {
        let err = StreamError::other("API Error: rate limit exceeded");
        assert!(!is_corrupted_thought_signature(&err));
    }

    #[test]
    fn ignores_retryable_errors() {
        let err = StreamError::transient("Corrupted thought signature.");
        assert!(!is_corrupted_thought_signature(&err));
    }

    /// Google validates thought signatures only within the current turn, so an
    /// earlier turn's signature is never the one it rejected and stripping it
    /// would leave the next request failing identically.
    #[test]
    fn builds_patch_for_signature_in_the_current_turn() {
        let req = request(vec![
            content(types::Role::User, vec![text_part("hello")]),
            content(types::Role::Model, vec![
                thought_part("thinking 1", "sig_previous_turn"),
                text_part("response 1"),
            ]),
            content(types::Role::User, vec![text_part("follow up")]),
            content(types::Role::Model, vec![
                thought_part("thinking 2", "sig_current_turn"),
                text_part("response 2"),
            ]),
        ]);

        let patches = build_thought_signature_patch(&req).unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].matcher, EventMatcher::MetadataValue {
            key: THOUGHT_SIGNATURE_KEY.to_owned(),
            value: "sig_current_turn".to_owned(),
        });
        assert_eq!(
            patches[0].action,
            PatchAction::RemoveMetadata(THOUGHT_SIGNATURE_KEY.to_owned())
        );
    }

    /// Nothing in the current turn can be stripped to fix the request, so the
    /// error is reported rather than answered with a patch that cannot help.
    #[test]
    fn returns_none_when_only_earlier_turns_are_signed() {
        let req = request(vec![
            content(types::Role::User, vec![text_part("hello")]),
            content(types::Role::Model, vec![thought_part(
                "thinking",
                "sig_previous_turn",
            )]),
            content(types::Role::User, vec![text_part("follow up")]),
            content(types::Role::Model, vec![text_part("unsigned answer")]),
        ]);

        assert!(build_thought_signature_patch(&req).is_none());
    }

    /// A turn spans every step of a tool-use loop: the function responses
    /// inside it are user messages, but they do not begin a new turn, so
    /// signatures from earlier steps stay in scope.
    #[test]
    fn function_responses_do_not_start_a_new_turn() {
        let req = request(vec![
            content(types::Role::User, vec![text_part("first prompt")]),
            content(types::Role::Model, vec![function_call_part(
                "old_tool",
                "sig_previous_turn",
            )]),
            content(types::Role::User, vec![function_response_part("old_tool")]),
            content(types::Role::Model, vec![text_part("answer")]),
            content(types::Role::User, vec![text_part("second prompt")]),
            content(types::Role::Model, vec![function_call_part(
                "step_one",
                "sig_step_one",
            )]),
            content(types::Role::User, vec![function_response_part("step_one")]),
            content(types::Role::Model, vec![function_call_part(
                "step_two",
                "sig_step_two",
            )]),
        ]);

        let patches = build_thought_signature_patch(&req).unwrap();
        assert_eq!(patches[0].matcher, EventMatcher::MetadataValue {
            key: THOUGHT_SIGNATURE_KEY.to_owned(),
            value: "sig_step_one".to_owned(),
        });
    }

    #[test]
    fn skips_dummy_signatures() {
        let req = request(vec![content(types::Role::Model, vec![
            types::ContentPart {
                data: types::ContentData::Text("text".to_owned()),
                thought: false,
                thought_signature: Some(THOUGHT_SIGNATURE_DUMMY_VALUE.to_owned()),
                metadata: None,
            },
        ])]);

        assert!(build_thought_signature_patch(&req).is_none());
    }

    #[test]
    fn returns_none_without_signatures() {
        let req = request(vec![
            content(types::Role::User, vec![text_part("hello")]),
            content(types::Role::Model, vec![text_part("world")]),
        ]);

        assert!(build_thought_signature_patch(&req).is_none());
    }

    #[test]
    fn returns_none_for_empty_request() {
        let req = request(vec![]);
        assert!(build_thought_signature_patch(&req).is_none());
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
    fn const_rewritten_to_enum() {
        let input = schema(json!({
          "type": "string",
          "const": "tool_call.my_tool.call_123"
        }));

        let out = transform_schema(input);

        assert_eq!(out.get("const"), None);
        assert_eq!(out["enum"], json!(["tool_call.my_tool.call_123"]));
        assert_eq!(out["type"], "string");
    }

    #[test]
    fn const_rewritten_for_non_string_values() {
        let input = schema(json!({
          "type": "integer",
          "const": 42
        }));

        let out = transform_schema(input);

        assert_eq!(out.get("const"), None);
        assert_eq!(out["enum"], json!([42]));
    }

    #[test]
    fn nested_const_in_properties_rewritten() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "inquiry_id": {
              "type": "string",
              "const": "tool_call.fs_modify_file.call_abc"
            },
            "answer": {
              "type": "boolean"
            }
          },
          "required": [
            "inquiry_id",
            "answer"
          ]
        }));

        let out = transform_schema(input);

        let inquiry_id = out["properties"]["inquiry_id"].as_object().unwrap();
        assert_eq!(inquiry_id.get("const"), None);
        assert_eq!(
            inquiry_id["enum"],
            json!(["tool_call.fs_modify_file.call_abc"])
        );
        assert_eq!(inquiry_id["type"], "string");
        assert_eq!(out["properties"]["answer"]["type"], "boolean");
    }

    #[test]
    fn deeply_nested_const_rewritten() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "outer": {
              "type": "object",
              "properties": {
                "inner": {
                  "type": "string",
                  "const": "fixed"
                }
              }
            }
          }
        }));

        let out = transform_schema(input);

        let inner = &out["properties"]["outer"]["properties"]["inner"];
        assert_eq!(inner.get("const"), None);
        assert_eq!(inner["enum"], json!(["fixed"]));
    }

    #[test]
    fn const_in_array_items_rewritten() {
        let input = schema(json!({
          "type": "array",
          "items": {
            "type": "string",
            "const": "only_value"
          }
        }));

        let out = transform_schema(input);

        let items = out["items"].as_object().unwrap();
        assert_eq!(items.get("const"), None);
        assert_eq!(items["enum"], json!(["only_value"]));
    }

    #[test]
    fn ref_inlined_from_defs() {
        let input = schema(json!({
          "type": "array",
          "items": {
            "$ref": "#/$defs/CountryInfo"
          },
          "$defs": {
            "CountryInfo": {
              "type": "object",
              "properties": {
                "continent": {
                  "type": "string"
                },
                "gdp": {
                  "type": "integer"
                }
              },
              "required": [
                "continent",
                "gdp"
              ]
            }
          }
        }));

        let out = transform_schema(input);

        // $defs should be removed from the output.
        assert!(out.get("$defs").is_none());

        // items should be the inlined definition.
        let items = out["items"].as_object().unwrap();
        assert_eq!(items["type"], "object");
        assert_eq!(items["properties"]["continent"]["type"], "string");
        assert_eq!(items["properties"]["gdp"]["type"], "integer");
        assert_eq!(items["required"], json!(["continent", "gdp"]));
    }

    #[test]
    fn ref_with_sibling_fields_preserved() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "person": {
              "$ref": "#/$defs/Person",
              "description": "The main person"
            }
          },
          "$defs": {
            "Person": {
              "type": "object",
              "properties": {
                "name": {
                  "type": "string"
                }
              }
            }
          }
        }));

        let out = transform_schema(input);

        // Sibling "description" should be preserved alongside inlined def.
        let person = out["properties"]["person"].as_object().unwrap();
        assert_eq!(person["type"], "object");
        assert_eq!(person["description"], "The main person");
        assert_eq!(person["properties"]["name"]["type"], "string");
    }

    #[test]
    fn ref_in_nested_property() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "addr": {
              "$ref": "#/$defs/Address"
            }
          },
          "$defs": {
            "Address": {
              "type": "object",
              "properties": {
                "city": {
                  "type": "string"
                },
                "zip": {
                  "type": "string"
                }
              }
            }
          }
        }));

        let out = transform_schema(input);

        let addr = out["properties"]["addr"].as_object().unwrap();
        assert_eq!(addr["type"], "object");
        assert_eq!(addr["properties"]["city"]["type"], "string");
        assert_eq!(addr["properties"]["zip"]["type"], "string");
        // Inlined object gets propertyOrdering.
        assert_eq!(addr["propertyOrdering"], json!(["city", "zip"]));
    }

    #[test]
    fn definitions_also_removed() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "x": {
              "$ref": "#/$defs/X"
            }
          },
          "definitions": {
            "X": {
              "type": "string"
            }
          }
        }));

        let out = transform_schema(input);

        assert!(out.get("definitions").is_none());
        assert_eq!(out["properties"]["x"]["type"], "string");
    }

    #[test]
    fn property_ordering_added_for_multiple_properties() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "first": {
              "type": "string"
            },
            "second": {
              "type": "integer"
            },
            "third": {
              "type": "boolean"
            }
          }
        }));

        let out = transform_schema(input);

        assert_eq!(out["propertyOrdering"], json!(["first", "second", "third"]));
    }

    #[test]
    fn property_ordering_not_added_for_single_property() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "only": {
              "type": "string"
            }
          }
        }));

        let out = transform_schema(input);

        assert!(out.get("propertyOrdering").is_none());
    }

    #[test]
    fn property_ordering_preserved_if_already_set() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "a": {
              "type": "string"
            },
            "b": {
              "type": "string"
            }
          },
          "propertyOrdering": [
            "b",
            "a"
          ]
        }));

        let out = transform_schema(input);

        // Existing ordering should not be overwritten.
        assert_eq!(out["propertyOrdering"], json!(["b", "a"]));
    }

    #[test]
    fn anyof_variants_processed() {
        let input = schema(json!({
          "anyOf": [
            {
              "type": "string",
              "const": "fixed"
            },
            {
              "type": "integer"
            }
          ]
        }));

        let out = transform_schema(input);

        let variants = out["anyOf"].as_array().unwrap();
        assert_eq!(variants.len(), 2);
        // const should be rewritten inside the variant.
        assert_eq!(variants[0]["enum"], json!(["fixed"]));
        assert!(variants[0].get("const").is_none());
        assert_eq!(variants[1]["type"], "integer");
    }

    #[test]
    fn anyof_with_ref_resolved() {
        let input = schema(json!({
          "anyOf": [
            {
              "$ref": "#/$defs/Str"
            },
            {
              "type": "integer"
            }
          ],
          "$defs": {
            "Str": {
              "type": "string"
            }
          }
        }));

        let out = transform_schema(input);

        let variants = out["anyOf"].as_array().unwrap();
        assert_eq!(variants[0]["type"], "string");
        assert_eq!(variants[1]["type"], "integer");
    }

    #[test]
    fn additional_properties_bool_preserved() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "name": {
              "type": "string"
            }
          },
          "additionalProperties": false
        }));

        let out = transform_schema(input);

        assert_eq!(out["additionalProperties"], json!(false));
    }

    #[test]
    fn additional_properties_schema_processed() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "name": {
              "type": "string"
            }
          },
          "additionalProperties": {
            "type": "string",
            "const": "extra"
          }
        }));

        let out = transform_schema(input);

        let additional = out["additionalProperties"].as_object().unwrap();
        assert_eq!(additional.get("const"), None);
        assert_eq!(additional["enum"], json!(["extra"]));
    }

    #[test]
    fn prefix_items_processed() {
        let input = schema(json!({
          "type": "array",
          "prefixItems": [
            {
              "type": "string",
              "const": "header"
            },
            {
              "type": "integer"
            }
          ]
        }));

        let out = transform_schema(input);

        let prefixes = out["prefixItems"].as_array().unwrap();
        assert_eq!(prefixes[0]["enum"], json!(["header"]));
        assert!(prefixes[0].get("const").is_none());
        assert_eq!(prefixes[1]["type"], "integer");
    }

    #[test]
    fn enum_preserved_unchanged() {
        let input = schema(json!({
          "type": "string",
          "enum": [
            "A",
            "B",
            "C"
          ]
        }));

        let out = transform_schema(input);

        assert_eq!(out["enum"], json!(["A", "B", "C"]));
    }

    #[test]
    fn supported_properties_preserved() {
        let input = schema(json!({
          "type": "integer",
          "minimum": 1,
          "maximum": 10,
          "description": "A number"
        }));

        let out = transform_schema(input);

        assert_eq!(out["type"], "integer");
        assert_eq!(out["minimum"], 1);
        assert_eq!(out["maximum"], 10);
        assert_eq!(out["description"], "A number");
    }

    /// The actual inquiry schema should transform correctly for Google.
    #[test]
    fn inquiry_schema_transforms_correctly() {
        let input = schema(json!({
          "type": "object",
          "properties": {
            "inquiry_id": {
              "type": "string",
              "const": "tool_call.fs_modify_file.call_a3b7c9d1"
            },
            "answer": {
              "type": "boolean"
            }
          },
          "required": [
            "inquiry_id",
            "answer"
          ],
          "additionalProperties": false
        }));

        let out = transform_schema(input);

        assert_eq!(
            Value::Object(out),
            json!({
                "type": "object",
                "required": ["inquiry_id", "answer"],
                "additionalProperties": false,
                "propertyOrdering": ["inquiry_id", "answer"],
                "properties": {
                    "inquiry_id": {
                        "type": "string",
                        "enum": ["tool_call.fs_modify_file.call_a3b7c9d1"]
                    },
                    "answer": {
                        "type": "boolean"
                    }
                }
            })
        );
    }

    /// The `title_schema` should pass through mostly unchanged.
    /// It has a single property so no `propertyOrdering` is added.
    #[test]
    fn title_schema_passes_through() {
        let input = crate::title::title_schema(3);
        let out = transform_schema(input.clone());

        assert_eq!(out, input);
    }

    /// Matches the example from the Python SDK docstring.
    #[test]
    fn sdk_docstring_example() {
        let input = schema(json!({
          "items": {
            "$ref": "#/$defs/CountryInfo"
          },
          "title": "Placeholder",
          "type": "array",
          "$defs": {
            "CountryInfo": {
              "properties": {
                "continent": {
                  "title": "Continent",
                  "type": "string"
                },
                "gdp": {
                  "title": "Gdp",
                  "type": "integer"
                }
              },
              "required": [
                "continent",
                "gdp"
              ],
              "title": "CountryInfo",
              "type": "object"
            }
          }
        }));

        let out = transform_schema(input);

        // $defs removed, $ref inlined, propertyOrdering added.
        assert_eq!(
            Value::Object(out),
            json!({
                "title": "Placeholder",
                "type": "array",
                "items": {
                    "properties": {
                        "continent": { "title": "Continent", "type": "string" },
                        "gdp": { "title": "Gdp", "type": "integer" }
                    },
                    "required": ["continent", "gdp"],
                    "title": "CountryInfo",
                    "type": "object",
                    "propertyOrdering": ["continent", "gdp"]
                }
            })
        );
    }
}
