mod make_schema_nullable {
    use serde_json::json;

    use super::super::make_schema_nullable;

    #[test]
    fn primitive_string_extends_type_array() {
        let mut schema = json!({ "type": "string" });

        make_schema_nullable(&mut schema);

        assert_eq!(schema, json!({ "type": ["string", "null"] }));
    }

    #[test]
    fn primitive_integer_extends_type_array() {
        let mut schema = json!({ "type": "integer", "description": "age" });

        make_schema_nullable(&mut schema);

        assert_eq!(
            schema,
            json!({ "type": ["integer", "null"], "description": "age" })
        );
    }

    #[test]
    fn array_uses_anyof_and_keeps_items_with_type() {
        // OpenAI's strict validator rejects `{type: ["array", "null"],
        // items: ...}` because `items` becomes orphaned from the array
        // variant of the type union. Use `anyOf` to keep `items` paired
        // with `type: array`.
        let mut schema = json!({
            "type": "array",
            "items": { "type": "string" }
        });

        make_schema_nullable(&mut schema);

        assert_eq!(
            schema,
            json!({
                "anyOf": [
                    { "type": "array", "items": { "type": "string" } },
                    { "type": "null" }
                ]
            })
        );
    }

    #[test]
    fn array_lifts_description_to_outer_wrapper() {
        let mut schema = json!({
            "type": "array",
            "items": { "type": "string" },
            "description": "list of paths"
        });

        make_schema_nullable(&mut schema);

        assert_eq!(
            schema,
            json!({
                "anyOf": [
                    { "type": "array", "items": { "type": "string" } },
                    { "type": "null" }
                ],
                "description": "list of paths"
            })
        );
    }

    #[test]
    fn object_uses_anyof_and_keeps_properties_with_type() {
        // Same rationale as nullable arrays: lifting `properties` out of
        // the type union via `anyOf` keeps them paired with `type: object`.
        let mut schema = json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        });

        make_schema_nullable(&mut schema);

        assert_eq!(
            schema,
            json!({
                "anyOf": [
                    {
                        "type": "object",
                        "properties": { "name": { "type": "string" } },
                        "required": ["name"]
                    },
                    { "type": "null" }
                ]
            })
        );
    }

    #[test]
    fn already_nullable_string_is_unchanged() {
        let mut schema = json!({ "type": ["string", "null"] });

        make_schema_nullable(&mut schema);

        assert_eq!(schema, json!({ "type": ["string", "null"] }));
    }

    #[test]
    fn already_anyof_nullable_array_is_unchanged() {
        // Idempotent: a schema that's already been made nullable via
        // anyOf shouldn't be re-wrapped.
        let mut schema = json!({
            "anyOf": [
                { "type": "array", "items": { "type": "string" } },
                { "type": "null" }
            ]
        });
        let original = schema.clone();

        make_schema_nullable(&mut schema);

        assert_eq!(schema, original);
    }
}

mod parameters_with_strict_mode {
    use indexmap::IndexMap;
    use jp_config::conversation::tool::{OneOrManyTypes, ToolParameterConfig};
    use serde_json::json;

    use super::super::parameters_with_strict_mode;

    fn cfg(kind: &str) -> ToolParameterConfig {
        ToolParameterConfig {
            kind: OneOrManyTypes::One(kind.to_owned()),
            default: None,
            required: false,
            summary: None,
            description: None,
            examples: None,
            enumeration: vec![],
            items: None,
            properties: IndexMap::default(),
        }
    }

    /// Regression for the `crate_search_items.kinds` schema rejected by
    /// OpenAI's strict validator with "array schema missing items" when
    /// the parameter is encoded as `{type: ["array", "null"], items: ...}`.
    #[test]
    fn nullable_array_parameter_renders_as_anyof() {
        let mut params = IndexMap::new();
        params.insert("crate_name".to_owned(), ToolParameterConfig {
            required: true,
            ..cfg("string")
        });
        params.insert("kinds".to_owned(), ToolParameterConfig {
            items: Some(Box::new(cfg("string"))),
            ..cfg("array")
        });

        let schema = parameters_with_strict_mode(params, true);

        let kinds = &schema["properties"]["kinds"];
        assert!(
            kinds.get("anyOf").is_some(),
            "nullable array must use anyOf form, got: {kinds}"
        );
        assert_eq!(
            kinds["anyOf"],
            json!([
                { "type": "array", "items": { "type": "string" } },
                { "type": "null" }
            ])
        );
        // Strict mode still requires every property in `required`.
        assert_eq!(schema["required"], json!(["crate_name", "kinds"]));
    }

    #[test]
    fn nullable_primitive_parameter_keeps_type_array_form() {
        let mut params = IndexMap::new();
        params.insert("label".to_owned(), cfg("string"));

        let schema = parameters_with_strict_mode(params, true);

        assert_eq!(
            schema["properties"]["label"]["type"],
            json!(["string", "null"])
        );
    }

    #[test]
    fn required_array_parameter_is_not_wrapped() {
        let mut params = IndexMap::new();
        params.insert("paths".to_owned(), ToolParameterConfig {
            required: true,
            items: Some(Box::new(cfg("string"))),
            ..cfg("array")
        });

        let schema = parameters_with_strict_mode(params, true);

        let paths = &schema["properties"]["paths"];
        assert_eq!(paths["type"], json!("array"));
        assert_eq!(paths["items"], json!({ "type": "string" }));
        assert!(paths.get("anyOf").is_none());
    }
}

mod ensure_strict_schema {
    use serde_json::{Value, json};

    use super::super::ensure_strict_schema;

    /// Helper to call `ensure_strict_schema` on a JSON value and return it.
    /// Lets test fixtures stay in their natural `json!({...})` form.
    fn run(mut schema: Value) -> Value {
        ensure_strict_schema(&mut schema);
        schema
    }

    #[test]
    fn no_required_adds_all_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            }
        });

        ensure_strict_schema(&mut schema);

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

        ensure_strict_schema(&mut schema);

        assert_eq!(schema["required"], json!(["old", "new", "paths"]));
        // old and new were already required, types unchanged.
        assert_eq!(schema["properties"]["old"]["type"], json!("string"));
        assert_eq!(schema["properties"]["new"]["type"], json!("string"));
        // paths was optional and is an array — nullability is encoded as
        // anyOf so that `items` stays paired with `type: array`.
        assert_eq!(
            schema["properties"]["paths"],
            json!({
                "anyOf": [
                    { "type": "array", "items": { "type": "string" } },
                    { "type": "null" }
                ]
            })
        );
    }

    #[test]
    fn nullable_object_property_uses_anyof() {
        // Latent twin of the nullable-array bug: a `{type: ["object",
        // "null"], properties: ...}` shape would orphan `properties`
        // for OpenAI's strict validator the same way `items` gets
        // orphaned for arrays. The inner object variant must also get
        // the full strict treatment (additionalProperties, required).
        let mut schema = json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "meta": {
                    "type": "object",
                    "properties": {
                        "author": { "type": "string" }
                    }
                }
            },
            "required": ["id"]
        });

        ensure_strict_schema(&mut schema);

        let meta = &schema["properties"]["meta"];
        let variants = meta["anyOf"].as_array().expect("anyOf array");
        assert_eq!(variants.len(), 2);
        // Inner object variant gets strict treatment via the recursion.
        assert_eq!(variants[0]["type"], json!("object"));
        assert_eq!(variants[0]["additionalProperties"], json!(false));
        assert_eq!(variants[0]["required"], json!(["author"]));
        // The inner object's `author` was newly required, so it's nullable.
        assert_eq!(
            variants[0]["properties"]["author"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(variants[1], json!({ "type": "null" }));
    }

    #[test]
    fn nested_object_in_properties_gets_strict_treatment() {
        // A non-nullable nested object inside `properties` must still
        // get `additionalProperties: false` and an expanded `required`
        // list. The recursion has to descend into each property value,
        // not just stop at the `properties` dict.
        let mut schema = json!({
            "type": "object",
            "properties": {
                "inner": {
                    "type": "object",
                    "properties": {
                        "x": { "type": "string" },
                        "y": { "type": "integer" }
                    },
                    "required": ["x"]
                }
            },
            "required": ["inner"]
        });

        ensure_strict_schema(&mut schema);

        let inner = &schema["properties"]["inner"];
        assert_eq!(inner["additionalProperties"], json!(false));
        assert_eq!(inner["required"], json!(["x", "y"]));
        // `x` was already required, stays as-is.
        assert_eq!(inner["properties"]["x"]["type"], json!("string"));
        // `y` was newly required, becomes nullable.
        assert_eq!(inner["properties"]["y"]["type"], json!(["integer", "null"]));
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

        ensure_strict_schema(&mut schema);

        assert_eq!(schema["required"], json!(["x", "y"]));
        // Both were already required, types stay as-is.
        assert_eq!(schema["properties"]["x"]["type"], json!("string"));
        assert_eq!(schema["properties"]["y"]["type"], json!("string"));
    }

    // ----- Tests below originally targeted the standalone
    // ----- `transform_schema` function for structured outputs. They now
    // ----- exercise the same merged `ensure_strict_schema` function
    // ----- through the `run()` helper.

    #[test]
    fn additional_properties_false_forced_on_objects() {
        let out = run(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        }));

        assert_eq!(out["additionalProperties"], json!(false));
    }

    #[test]
    fn additional_properties_true_overridden_to_false() {
        let out = run(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "additionalProperties": true
        }));

        assert_eq!(out["additionalProperties"], json!(false));
    }

    #[test]
    fn existing_required_expanded_with_nullable_optionals() {
        // OpenAI strict mode requires all properties in `required`,
        // but the docs explicitly say optional fields are emulated
        // via a union with null. We preserve the user's intent by
        // making previously-optional fields nullable rather than
        // silently demanding the model emit a value for them.
        let out = run(json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            },
            "required": ["name"]
        }));

        assert_eq!(out["required"], json!(["name", "age"]));
        // `name` was already required, type unchanged.
        assert_eq!(out["properties"]["name"]["type"], json!("string"));
        // `age` was newly required, made nullable so the model can
        // emit null to omit it.
        assert_eq!(out["properties"]["age"]["type"], json!(["integer", "null"]));
    }

    // Note: a non-required nested object test is covered above by
    // `nested_object_in_properties_gets_strict_treatment`, which marks
    // `inner` as required so the strict treatment lands at the
    // expected level rather than inside an `anyOf` wrapper.

    #[test]
    fn defs_recursively_processed() {
        let out = run(json!({
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

        // $defs is kept (OpenAI supports it natively).
        let step_def = &out["$defs"]["Step"];
        assert_eq!(step_def["additionalProperties"], json!(false));
        assert_eq!(step_def["required"], json!(["explanation"]));
    }

    #[test]
    fn standalone_ref_kept_as_is() {
        let out = run(json!({
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

        // Standalone $ref should stay.
        assert_eq!(out["properties"]["step"]["$ref"], "#/$defs/Step");
    }

    #[test]
    fn ref_with_siblings_unraveled() {
        // `required: ["name"]` on the Person def keeps the test
        // focused on $ref unraveling — without it, `name` would also
        // be made nullable, which is correct but tangential to this
        // test's purpose.
        let out = run(json!({
            "type": "object",
            "properties": {
                "person": {
                    "$ref": "#/$defs/Person",
                    "description": "The main person"
                }
            },
            "required": ["person"],
            "$defs": {
                "Person": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" }
                    },
                    "required": ["name"]
                }
            }
        }));

        let person = &out["properties"]["person"];
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
        let out = run(json!({
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

        let variants = out["properties"]["item"]["anyOf"].as_array().unwrap();
        let obj_variant = &variants[0];
        assert_eq!(obj_variant["additionalProperties"], json!(false));
        assert_eq!(obj_variant["required"], json!(["name"]));
    }

    #[test]
    fn allof_single_element_merged() {
        let out = run(json!({
            "allOf": [{
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                }
            }]
        }));

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
        let out = run(json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "id": { "type": "integer" }
                }
            }
        }));

        let items = &out["items"];
        assert_eq!(items["additionalProperties"], json!(false));
        assert_eq!(items["required"], json!(["id"]));
    }

    /// The inquiry schema should get strict treatment.
    #[test]
    fn inquiry_schema_transforms_correctly() {
        let out = run(json!({
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
        let mut input = Value::Object(crate::title::title_schema(3));
        ensure_strict_schema(&mut input);

        assert_eq!(input["additionalProperties"], json!(false));
        assert_eq!(input["required"], json!(["titles"]));

        let titles = &input["properties"]["titles"];
        let items = &titles["items"];
        assert_eq!(items["type"], "string");
    }

    /// Docs example: definitions with $ref. Verbatim from
    /// <https://platform.openai.com/docs/guides/structured-outputs>.
    #[test]
    fn definitions_example_from_docs() {
        let out = run(json!({
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
                    "required": ["explanation", "output"],
                    "additionalProperties": false
                }
            },
            "required": ["steps", "final_answer"],
            "additionalProperties": false
        }));

        // Root object: all properties required.
        assert_eq!(out["required"], json!(["steps", "final_answer"]));

        // $defs preserved, step def gets required added.
        let step_def = &out["$defs"]["step"];
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

    /// The exact in-stream overload payload reported in the field:
    /// `type=service_unavailable_error, code=server_is_overloaded`. This used
    /// to surface as a non-retryable `Other` error because the message has no
    /// parseable retry-after hint.
    #[test]
    fn service_unavailable_overload_is_classified_as_transient() {
        let e = err(
            "service_unavailable_error",
            Some("server_is_overloaded"),
            "Our servers are currently overloaded. Please try again later.",
        );

        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::Transient);
        assert!(classified.is_retryable());
    }

    #[test]
    fn overloaded_error_type_is_classified_as_transient() {
        let e = err("overloaded_error", None, "backend overloaded");
        let classified = classify_stream_error(e);

        assert_eq!(classified.kind, StreamErrorKind::Transient);
        assert!(classified.is_retryable());
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
