mod enforce_strict_object_structure {
    use serde_json::json;

    use super::super::enforce_strict_object_structure;

    #[test]
    fn no_required_adds_all_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            }
        });

        enforce_strict_object_structure(&mut schema);

        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["required"], json!(["name", "age"]));
        // Both were newly required, so both become nullable.
        assert_eq!(
            schema["properties"]["name"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(
            schema["properties"]["age"]["type"],
            json!(["integer", "null"])
        );
    }

    #[test]
    fn partial_required_expanded_and_optional_made_nullable() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "old": { "type": "string" },
                "new": { "type": "string" },
                "paths": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["old", "new"]
        });

        enforce_strict_object_structure(&mut schema);

        assert_eq!(schema["required"], json!(["old", "new", "paths"]));
        // old and new were already required, types unchanged.
        assert_eq!(schema["properties"]["old"]["type"], json!("string"));
        assert_eq!(schema["properties"]["new"]["type"], json!("string"));
        // paths was optional, now nullable.
        assert_eq!(
            schema["properties"]["paths"]["type"],
            json!(["array", "null"])
        );
    }

    #[test]
    fn already_nullable_not_doubled() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "value": { "type": ["string", "null"] }
            }
        });

        enforce_strict_object_structure(&mut schema);

        // Should not add a second "null".
        assert_eq!(
            schema["properties"]["value"]["type"],
            json!(["string", "null"])
        );
    }

    #[test]
    fn nested_object_in_array_items() {
        let mut schema = json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "a": { "type": "string" },
                    "b": { "type": "integer" }
                },
                "required": ["a"]
            }
        });

        enforce_strict_object_structure(&mut schema);

        let items = &schema["items"];
        assert_eq!(items["required"], json!(["a", "b"]));
        assert_eq!(items["properties"]["a"]["type"], json!("string"));
        assert_eq!(items["properties"]["b"]["type"], json!(["integer", "null"]));
    }

    #[test]
    fn all_required_stays_unchanged() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "x": { "type": "string" },
                "y": { "type": "string" }
            },
            "required": ["x", "y"]
        });

        enforce_strict_object_structure(&mut schema);

        assert_eq!(schema["required"], json!(["x", "y"]));
        // Both were already required, types stay as-is.
        assert_eq!(schema["properties"]["x"]["type"], json!("string"));
        assert_eq!(schema["properties"]["y"]["type"], json!("string"));
    }
}

mod transform_schema {
    use serde_json::{Map, Value, json};

    use super::super::transform_schema;

    #[expect(clippy::needless_pass_by_value)]
    fn schema(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn additional_properties_false_forced_on_objects() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        }));

        let out = transform_schema(input);

        assert_eq!(out["additionalProperties"], json!(false));
    }

    #[test]
    fn additional_properties_true_overridden_to_false() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "additionalProperties": true
        }));

        let out = transform_schema(input);

        assert_eq!(out["additionalProperties"], json!(false));
    }

    #[test]
    fn all_properties_forced_into_required() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            }
        }));

        let out = transform_schema(input);

        assert_eq!(out["required"], json!(["name", "age"]));
    }

    #[test]
    fn existing_required_overwritten_to_all_properties() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            },
            "required": ["name"]
        }));

        let out = transform_schema(input);

        // Both properties must be required in strict mode.
        assert_eq!(out["required"], json!(["name", "age"]));
    }

    #[test]
    fn nested_objects_get_strict_treatment() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "inner": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "string" }
                    }
                }
            }
        }));

        let out = transform_schema(input);

        let inner = out["properties"]["inner"].as_object().unwrap();
        assert_eq!(inner["additionalProperties"], json!(false));
        assert_eq!(inner["required"], json!(["x"]));
    }

    #[test]
    fn defs_recursively_processed() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "step": { "$ref": "#/$defs/Step" }
            },
            "$defs": {
                "Step": {
                    "type": "object",
                    "properties": {
                        "explanation": { "type": "string" }
                    }
                }
            }
        }));

        let out = transform_schema(input);

        // $defs is kept (OpenAI supports it natively).
        let step_def = out["$defs"]["Step"].as_object().unwrap();
        assert_eq!(step_def["additionalProperties"], json!(false));
        assert_eq!(step_def["required"], json!(["explanation"]));
    }

    #[test]
    fn standalone_ref_kept_as_is() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "step": { "$ref": "#/$defs/Step" }
            },
            "$defs": {
                "Step": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                }
            }
        }));

        let out = transform_schema(input);

        // Standalone $ref should stay.
        assert_eq!(out["properties"]["step"]["$ref"], "#/$defs/Step");
    }

    #[test]
    fn ref_with_siblings_unraveled() {
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
                        "name": { "type": "string" }
                    }
                }
            }
        }));

        let out = transform_schema(input);

        let person = out["properties"]["person"].as_object().unwrap();
        // $ref should be removed, definition inlined.
        assert!(person.get("$ref").is_none());
        assert_eq!(person["type"], "object");
        assert_eq!(person["description"], "The main person");
        assert_eq!(person["properties"]["name"]["type"], "string");
        // Inlined object should also get strict treatment.
        assert_eq!(person["additionalProperties"], json!(false));
    }

    #[test]
    fn anyof_variants_processed() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "item": {
                    "anyOf": [
                        {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string" }
                            }
                        },
                        { "type": "string" }
                    ]
                }
            }
        }));

        let out = transform_schema(input);

        let variants = out["properties"]["item"]["anyOf"].as_array().unwrap();
        let obj_variant = variants[0].as_object().unwrap();
        assert_eq!(obj_variant["additionalProperties"], json!(false));
        assert_eq!(obj_variant["required"], json!(["name"]));
    }

    #[test]
    fn allof_single_element_merged() {
        let input = schema(json!({
            "allOf": [{
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                }
            }]
        }));

        let out = transform_schema(input);

        assert!(out.get("allOf").is_none());
        assert_eq!(out["type"], "object");
        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["name"]));
    }

    #[test]
    fn allof_multiple_elements_merged() {
        let input = schema(json!({
            "allOf": [
                {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    }
                },
                {
                    "description": "Extra info"
                }
            ]
        }));

        let out = transform_schema(input);

        assert!(out.get("allOf").is_none());
        assert_eq!(out["type"], "object");
        assert_eq!(out["description"], "Extra info");
    }

    #[test]
    fn null_default_stripped() {
        let input = schema(json!({
            "type": "string",
            "default": null
        }));

        let out = transform_schema(input);

        assert!(out.get("default").is_none());
    }

    #[test]
    fn non_null_default_preserved() {
        let input = schema(json!({
            "type": "string",
            "default": "hello"
        }));

        let out = transform_schema(input);

        assert_eq!(out["default"], "hello");
    }

    #[test]
    fn const_preserved_unchanged() {
        let input = schema(json!({
            "type": "string",
            "const": "tool_call.my_tool.call_123"
        }));

        let out = transform_schema(input);

        assert_eq!(out["const"], "tool_call.my_tool.call_123");
    }

    #[test]
    fn array_items_recursively_processed() {
        let input = schema(json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" }
                }
            }
        }));

        let out = transform_schema(input);

        let items = out["items"].as_object().unwrap();
        assert_eq!(items["additionalProperties"], json!(false));
        assert_eq!(items["required"], json!(["id"]));
    }

    /// The inquiry schema should get strict treatment.
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
            "required": ["inquiry_id", "answer"],
            "additionalProperties": false
        }));

        let out = transform_schema(input);

        // const should be preserved (OpenAI supports it).
        assert_eq!(
            out["properties"]["inquiry_id"]["const"],
            "tool_call.fs_modify_file.call_a3b7c9d1"
        );
        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["inquiry_id", "answer"]));
    }

    /// The `title_schema` should get strict treatment applied.
    #[test]
    fn title_schema_gets_strict_treatment() {
        let input = crate::title::title_schema(3);
        let out = transform_schema(input);

        assert_eq!(out["additionalProperties"], json!(false));
        assert_eq!(out["required"], json!(["titles"]));

        let titles = out["properties"]["titles"].as_object().unwrap();
        let items = titles["items"].as_object().unwrap();
        assert_eq!(items["type"], "string");
    }

    /// Docs example: definitions with $ref.
    #[test]
    fn definitions_example_from_docs() {
        let input = schema(json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/step" }
                },
                "final_answer": { "type": "string" }
            },
            "$defs": {
                "step": {
                    "type": "object",
                    "properties": {
                        "explanation": { "type": "string" },
                        "output": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        }));

        let out = transform_schema(input);

        // Root object: all properties required.
        assert_eq!(out["required"], json!(["steps", "final_answer"]));

        // $defs preserved, step def gets required added.
        let step_def = out["$defs"]["step"].as_object().unwrap();
        assert_eq!(step_def["required"], json!(["explanation", "output"]));
        assert_eq!(step_def["additionalProperties"], json!(false));

        // $ref stays as-is (standalone, no siblings).
        assert_eq!(out["properties"]["steps"]["items"]["$ref"], "#/$defs/step");
    }
}

mod map_model {
    use chrono::{TimeZone as _, Utc};

    use super::super::{
        ModelResponse, STREAMING_UNSUPPORTED, TEMP_REQUIRES_NO_REASONING, map_model,
    };
    use crate::model::{ModelDeprecation, ReasoningDetails};

    fn model(id: &str) -> ModelResponse {
        ModelResponse {
            id: id.to_owned(),
            _object: "model".to_owned(),
            _created: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
            _owned_by: "openai".to_owned(),
        }
    }

    #[test]
    fn gpt_5_5_uses_latest_metadata() {
        let details = map_model(model("gpt-5.5")).unwrap();

        assert_eq!(details.display_name.as_deref(), Some("GPT-5.5"));
        assert_eq!(details.context_window, Some(1_050_000));
        assert_eq!(details.max_output_tokens, Some(128_000));
        assert_eq!(
            details.reasoning,
            Some(ReasoningDetails::leveled(
                true, false, true, true, true, true,
            ))
        );
        assert_eq!(
            details.knowledge_cutoff,
            chrono::NaiveDate::from_ymd_opt(2025, 12, 1)
        );
        assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
        assert_eq!(details.features, vec![TEMP_REQUIRES_NO_REASONING]);
    }

    #[test]
    fn gpt_5_5_pro_marks_streaming_as_unsupported() {
        let details = map_model(model("gpt-5.5-pro")).unwrap();

        assert_eq!(details.display_name.as_deref(), Some("GPT-5.5 pro"));
        assert_eq!(details.context_window, Some(1_050_000));
        assert_eq!(details.max_output_tokens, Some(128_000));
        assert_eq!(
            details.reasoning,
            Some(ReasoningDetails::leveled(
                false, false, false, true, true, true,
            ))
        );
        assert_eq!(
            details.knowledge_cutoff,
            chrono::NaiveDate::from_ymd_opt(2025, 12, 1)
        );
        assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
        assert_eq!(details.features, vec![
            TEMP_REQUIRES_NO_REASONING,
            STREAMING_UNSUPPORTED
        ]);
    }
}

mod synthesize_non_streaming_output_item_events {
    use openai_responses::types;
    use serde_json::{Map, json};

    use super::super::{
        ENCRYPTED_CONTENT_KEY, ITEM_ID_KEY, PHASE_KEY, map_event,
        synthesize_non_streaming_output_item_events,
    };
    use crate::event::Event;

    fn output_item(value: serde_json::Value) -> types::OutputItem {
        serde_json::from_value(value).unwrap()
    }

    fn collect_events(
        index: usize,
        item: types::OutputItem,
        is_structured: bool,
        reasoning_enabled: bool,
    ) -> Vec<Event> {
        synthesize_non_streaming_output_item_events(index, item)
            .into_iter()
            .flat_map(|event| map_event(event, is_structured, reasoning_enabled))
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
    }

    #[test]
    fn message_item_emits_message_and_flush_metadata() {
        let events = collect_events(
            0,
            output_item(json!({
                "type": "message",
                "id": "msg_123",
                "status": "completed",
                "role": "assistant",
                "phase": "final_answer",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": [],
                    "text": "hello"
                }]
            })),
            false,
            true,
        );

        assert_eq!(events, vec![
            Event::message(0, ""),
            Event::message(0, "hello"),
            Event::flush_with_metadata(
                0,
                Map::from_iter([
                    (PHASE_KEY.to_owned(), "final_answer".into()),
                    (ITEM_ID_KEY.to_owned(), "msg_123".into()),
                ])
            ),
        ]);
    }

    #[test]
    fn refusal_item_reuses_message_streaming_path() {
        let events = collect_events(
            0,
            output_item(json!({
                "type": "message",
                "id": "msg_456",
                "status": "completed",
                "role": "assistant",
                "content": [{
                    "type": "refusal",
                    "refusal": "nope"
                }]
            })),
            false,
            true,
        );

        assert_eq!(events, vec![
            Event::message(0, ""),
            Event::message(0, "nope"),
            Event::flush_with_metadata(
                0,
                Map::from_iter([(ITEM_ID_KEY.to_owned(), "msg_456".into())])
            ),
        ]);
    }

    #[test]
    fn reasoning_item_emits_empty_reasoning_and_metadata() {
        let events = collect_events(
            1,
            output_item(json!({
                "type": "reasoning",
                "id": "rs_123",
                "status": "completed",
                "summary": [],
                "encrypted_content": "enc"
            })),
            false,
            true,
        );

        assert_eq!(events, vec![
            Event::reasoning(1, ""),
            Event::flush_with_metadata(
                1,
                Map::from_iter([
                    (ITEM_ID_KEY.to_owned(), "rs_123".into()),
                    (ENCRYPTED_CONTENT_KEY.to_owned(), "enc".into()),
                ])
            ),
        ]);
    }

    #[test]
    fn reasoning_item_is_skipped_when_reasoning_is_disabled() {
        let events = collect_events(
            1,
            output_item(json!({
                "type": "reasoning",
                "id": "rs_123",
                "status": "completed",
                "summary": [],
                "encrypted_content": "enc"
            })),
            false,
            false,
        );

        assert!(events.is_empty());
    }

    #[test]
    fn function_call_item_emits_tool_call_events() {
        let events = collect_events(
            2,
            output_item(json!({
                "type": "function_call",
                "id": "fc_123",
                "status": "completed",
                "arguments": "{\"foo\":\"bar\"}",
                "call_id": "call_123",
                "name": "run_me"
            })),
            false,
            true,
        );

        assert_eq!(events, vec![
            Event::tool_call_start(2, "call_123", "run_me"),
            Event::tool_call_args(2, "{\"foo\":\"bar\"}"),
            Event::flush(2),
        ]);
    }
}

mod map_non_streaming_finish_reason {
    use openai_responses::types;

    use super::super::map_non_streaming_finish_reason;
    use crate::event::{Event, FinishReason};

    #[test]
    fn completed_status_maps_to_completed() {
        let event =
            map_non_streaming_finish_reason(types::ResponseStatus::Completed, None).unwrap();

        assert_eq!(event, Event::Finished(FinishReason::Completed));
    }

    #[test]
    fn incomplete_max_output_tokens_maps_to_max_tokens() {
        let event = map_non_streaming_finish_reason(
            types::ResponseStatus::Incomplete,
            Some("max_output_tokens".to_owned()),
        )
        .unwrap();

        assert_eq!(event, Event::Finished(FinishReason::MaxTokens));
    }
}

mod classify_stream_error {
    use std::time::Duration;

    use openai_responses::types::response;

    use super::super::classify_stream_error;
    use crate::error::StreamErrorKind;

    fn err(type_: &str, code: Option<&str>, message: &str) -> response::Error {
        response::Error {
            r#type: type_.to_owned(),
            code: code.map(str::to_owned),
            message: message.to_owned(),
            param: None,
        }
    }

    /// The exact in-stream rate-limit payload reported in the field:
    /// `type=tokens, code=rate_limit_exceeded`, with a `try again in 2.398s.`
    /// hint. This used to surface as a non-retryable `Other` error.
    #[test]
    fn tpm_rate_limit_with_code_is_classified_as_rate_limit() {
        let e = err(
            "tokens",
            Some("rate_limit_exceeded"),
            "Rate limit reached for gpt-5.4 (for limit gpt-5.4-long-context) in \
             organization org-xxx on tokens per min (TPM): Limit 2000000, Used \
             1354056, Requested 725891. Please try again in 2.398s. Visit \
             https://platform.openai.com/account/rate-limits to learn more.",
        );

        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::RateLimit);
        assert_eq!(classified.retry_after, Some(Duration::from_secs(3)));
        assert!(classified.is_retryable());
    }

    #[test]
    fn rate_limit_via_type_field() {
        let e = err("rate_limit_exceeded", None, "too many requests");
        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::RateLimit);
        assert!(classified.is_retryable());
    }

    #[test]
    fn insufficient_quota_via_code() {
        let e = err("tokens", Some("insufficient_quota"), "out of credits");
        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::InsufficientQuota);
        assert!(!classified.is_retryable());
    }

    #[test]
    fn server_error_carries_retry_after_hint() {
        let e = err(
            "server_error",
            None,
            "Upstream hiccup, please try again in 10s.",
        );
        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::Transient);
        assert_eq!(classified.retry_after, Some(Duration::from_secs(10)));
    }

    #[test]
    fn unknown_type_with_retry_hint_becomes_transient() {
        let e = err("something_new", None, "Please try again in 5s.");
        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::Transient);
        assert_eq!(classified.retry_after, Some(Duration::from_secs(5)));
        assert!(classified.is_retryable());
    }

    #[test]
    fn unknown_type_without_retry_hint_stays_other() {
        let e = err("something_new", None, "unexpected");
        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::Other);
        assert_eq!(classified.retry_after, None);
    }
}
