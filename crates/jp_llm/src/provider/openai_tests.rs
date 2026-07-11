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
    /// OpenAI's strict validator with "array schema missing items" when the
    /// parameter is encoded as `{type: ["array", "null"], items: ...}`.
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

        ensure_strict_schema(&mut schema);

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

        ensure_strict_schema(&mut schema);

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
        let out = run(json!({
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
        let out = run(json!({
            "type": "string",
            "default": null
        }));

        assert!(out.get("default").is_none());
    }

    #[test]
    fn non_null_default_preserved() {
        let out = run(json!({
            "type": "string",
            "default": "hello"
        }));

        assert_eq!(out["default"], "hello");
    }

    #[test]
    fn const_preserved_unchanged() {
        let out = run(json!({
            "type": "string",
            "const": "tool_call.my_tool.call_123"
        }));

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

    /// Docs example: definitions with $ref.
    /// Verbatim from
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
        EXPLICIT_PROMPT_CACHING, ModelResponse, PERSISTED_REASONING, REASONING_PRO_MODE,
        STREAMING_UNSUPPORTED, TEMP_REQUIRES_NO_REASONING, map_model,
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
    fn gpt_5_6_sol_uses_latest_metadata() {
        let details = map_model(model("gpt-5.6-sol")).unwrap();

        assert_eq!(details.display_name.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(details.context_window, Some(1_050_000));
        assert_eq!(details.max_output_tokens, Some(128_000));
        assert_eq!(
            details.reasoning,
            Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, true,
            ))
        );
        assert_eq!(
            details.knowledge_cutoff,
            chrono::NaiveDate::from_ymd_opt(2026, 2, 16)
        );
        assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
        assert_eq!(details.features, vec![
            TEMP_REQUIRES_NO_REASONING,
            REASONING_PRO_MODE,
            PERSISTED_REASONING,
            EXPLICIT_PROMPT_CACHING
        ]);
    }

    #[test]
    fn gpt_5_6_alias_resolves_to_sol_metadata() {
        let details = map_model(model("gpt-5.6")).unwrap();

        assert_eq!(details.display_name.as_deref(), Some("GPT-5.6 Sol"));
    }

    #[test]
    fn gpt_5_6_terra_uses_latest_metadata() {
        let details = map_model(model("gpt-5.6-terra")).unwrap();

        assert_eq!(details.display_name.as_deref(), Some("GPT-5.6 Terra"));
        assert_eq!(details.context_window, Some(1_050_000));
        assert_eq!(details.max_output_tokens, Some(128_000));
        assert_eq!(
            details.reasoning,
            Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, true,
            ))
        );
        assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
        assert_eq!(details.features, vec![
            TEMP_REQUIRES_NO_REASONING,
            REASONING_PRO_MODE,
            PERSISTED_REASONING,
            EXPLICIT_PROMPT_CACHING
        ]);
    }

    #[test]
    fn gpt_5_6_luna_uses_latest_metadata() {
        let details = map_model(model("gpt-5.6-luna")).unwrap();

        assert_eq!(details.display_name.as_deref(), Some("GPT-5.6 Luna"));
        assert_eq!(details.context_window, Some(1_050_000));
        assert_eq!(details.max_output_tokens, Some(128_000));
        assert_eq!(
            details.reasoning,
            Some(ReasoningDetails::leveled(
                true, false, true, true, true, true, true,
            ))
        );
        assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
        assert_eq!(details.features, vec![
            TEMP_REQUIRES_NO_REASONING,
            REASONING_PRO_MODE,
            PERSISTED_REASONING,
            EXPLICIT_PROMPT_CACHING
        ]);
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
                true, false, true, true, true, true, false,
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
                false, false, false, true, true, true, false,
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

mod unknown_model {
    use chrono::{TimeZone as _, Utc};
    use jp_config::{
        AppConfig,
        model::{
            id::{ModelIdConfig, ModelIdOrAliasConfig, ProviderId},
            parameters::ReasoningConfig,
        },
        providers::llm::LlmProviderConfig,
    };
    use jp_conversation::{
        ConversationStream,
        event::{ChatRequest, ChatResponse, ConversationEvent, TurnStart},
        thread::ThreadBuilder,
    };

    use super::super::{ENCRYPTED_CONTENT_KEY, ITEM_ID_KEY};
    use crate::{
        model::{ModelDetails, ReasoningDetails},
        provider::build_request_value,
        query::ChatQuery,
    };

    /// A model absent from the catalog (e.g. released after this binary was
    /// built) replays stored reasoning events that carry OpenAI item ids as
    /// native reasoning items — not as `<think>` fallback text, which the
    /// model would mimic in its visible output.
    #[test]
    fn replays_reasoning_natively() {
        let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

        let mut config = AppConfig::new_test();
        config.assistant.model.id = ModelIdOrAliasConfig::Id(ModelIdConfig {
            provider: ProviderId::Openai,
            name: "gpt-9".parse().unwrap(),
        });

        let mut stream = ConversationStream::new(config.into()).with_created_at(ts);
        stream.extend([
            ConversationEvent::new(TurnStart, ts),
            ConversationEvent::new(ChatRequest::from("First question"), ts),
            ConversationEvent::new(ChatResponse::reasoning("Earlier reasoning."), ts)
                .with_metadata_field(ITEM_ID_KEY, "rs_123")
                .with_metadata_field(ENCRYPTED_CONTENT_KEY, "encrypted-blob"),
            ConversationEvent::new(ChatResponse::message("First answer."), ts),
            ConversationEvent::new(TurnStart, ts),
            ConversationEvent::new(ChatRequest::from("Second question"), ts),
        ]);

        let thread = ThreadBuilder::new().with_events(stream).build().unwrap();

        // Dummy API key env var, mirroring the VCR harness.
        let env = if cfg!(windows) { "USERNAME" } else { "USER" }.to_owned();
        let mut providers = LlmProviderConfig::default();
        providers.openai.api_key_env = env;

        let model = ModelDetails::empty("openai/gpt-9".parse().unwrap());
        let request = build_request_value(
            ProviderId::Openai,
            &providers,
            &model,
            ChatQuery::from(thread),
        )
        .unwrap()
        .to_string();

        assert!(
            !request.contains("<think>"),
            "reasoning replayed as fallback text: {request}"
        );
        assert!(
            request.contains(r#""type":"reasoning""#) && request.contains("encrypted-blob"),
            "native reasoning item missing from request: {request}"
        );
    }

    /// OpenAI item provenance wins over target-model capability metadata.
    /// A native reasoning item remains native even when continuing with a model
    /// cataloged as not supporting reasoning; the API owns compatibility.
    #[test]
    fn native_reasoning_replay_ignores_target_capability() {
        let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

        let mut config = AppConfig::new_test();
        config.assistant.model.id = ModelIdOrAliasConfig::Id(ModelIdConfig {
            provider: ProviderId::Openai,
            name: "gpt-9".parse().unwrap(),
        });

        let mut stream = ConversationStream::new(config.into()).with_created_at(ts);
        stream.extend([
            ConversationEvent::new(TurnStart, ts),
            ConversationEvent::new(ChatRequest::from("First question"), ts),
            ConversationEvent::new(ChatResponse::reasoning("Earlier reasoning."), ts)
                .with_metadata_field(ITEM_ID_KEY, "rs_123")
                .with_metadata_field(ENCRYPTED_CONTENT_KEY, "encrypted-blob"),
            ConversationEvent::new(ChatResponse::message("First answer."), ts),
            ConversationEvent::new(TurnStart, ts),
            ConversationEvent::new(ChatRequest::from("Second question"), ts),
        ]);

        let thread = ThreadBuilder::new().with_events(stream).build().unwrap();
        let env = if cfg!(windows) { "USERNAME" } else { "USER" }.to_owned();
        let mut providers = LlmProviderConfig::default();
        providers.openai.api_key_env = env;

        let mut model = ModelDetails::empty("openai/gpt-4o".parse().unwrap());
        model.reasoning = Some(ReasoningDetails::unsupported());
        let request = build_request_value(
            ProviderId::Openai,
            &providers,
            &model,
            ChatQuery::from(thread),
        )
        .unwrap()
        .to_string();

        assert!(
            request.contains(r#""type":"reasoning""#) && request.contains("encrypted-blob"),
            "native reasoning item missing from request: {request}"
        );
        assert!(!request.contains("<think>"));
    }

    /// Active reasoning on a model absent from the catalog strips `temperature`
    /// and `top_p`: unknown models are newer than this binary, and every
    /// GPT-5-era model rejects sampling parameters alongside active reasoning.
    #[test]
    fn active_reasoning_strips_temperature() {
        let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

        let mut config = AppConfig::new_test();
        config.assistant.model.id = ModelIdOrAliasConfig::Id(ModelIdConfig {
            provider: ProviderId::Openai,
            name: "gpt-9".parse().unwrap(),
        });
        config.assistant.model.parameters.reasoning = Some(ReasoningConfig::Auto);
        config.assistant.model.parameters.temperature = Some(0.7);
        config.assistant.model.parameters.top_p = Some(0.9);

        let mut stream = ConversationStream::new(config.into()).with_created_at(ts);
        stream.extend([
            ConversationEvent::new(TurnStart, ts),
            ConversationEvent::new(ChatRequest::from("A question"), ts),
        ]);

        let thread = ThreadBuilder::new().with_events(stream).build().unwrap();

        // Dummy API key env var, mirroring the VCR harness.
        let env = if cfg!(windows) { "USERNAME" } else { "USER" }.to_owned();
        let mut providers = LlmProviderConfig::default();
        providers.openai.api_key_env = env;

        let model = ModelDetails::empty("openai/gpt-9".parse().unwrap());
        let request = build_request_value(
            ProviderId::Openai,
            &providers,
            &model,
            ChatQuery::from(thread),
        )
        .unwrap()
        .to_string();

        assert!(
            request.contains(r#""temperature":null"#),
            "temperature not stripped alongside active reasoning: {request}"
        );
        assert!(
            request.contains(r#""top_p":null"#),
            "top_p not stripped alongside active reasoning: {request}"
        );
    }

    /// `reasoning = "off"` on a model absent from the catalog disables
    /// reasoning explicitly with `effort: none`.
    /// Omitting the field would let the model reason (hidden, and billed) at
    /// its default effort.
    #[test]
    fn reasoning_off_sends_none_effort() {
        let ts = Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

        let mut config = AppConfig::new_test();
        config.assistant.model.id = ModelIdOrAliasConfig::Id(ModelIdConfig {
            provider: ProviderId::Openai,
            name: "gpt-9".parse().unwrap(),
        });
        config.assistant.model.parameters.reasoning = Some(ReasoningConfig::Off);

        let mut stream = ConversationStream::new(config.into()).with_created_at(ts);
        stream.extend([
            ConversationEvent::new(TurnStart, ts),
            ConversationEvent::new(ChatRequest::from("A question"), ts),
        ]);

        let thread = ThreadBuilder::new().with_events(stream).build().unwrap();

        // Dummy API key env var, mirroring the VCR harness.
        let env = if cfg!(windows) { "USERNAME" } else { "USER" }.to_owned();
        let mut providers = LlmProviderConfig::default();
        providers.openai.api_key_env = env;

        let model = ModelDetails::empty("openai/gpt-9".parse().unwrap());
        let request = build_request_value(
            ProviderId::Openai,
            &providers,
            &model,
            ChatQuery::from(thread),
        )
        .unwrap()
        .to_string();

        assert!(
            request.contains(r#""effort":"none""#),
            "reasoning not explicitly disabled: {request}"
        );
    }
}

mod convert_reasoning {
    use jp_config::model::parameters::{CustomReasoningConfig, ReasoningEffort};
    use openai_responses::types;

    use super::super::convert_reasoning;
    use crate::model::{ModelDetails, ReasoningDetails};

    fn model(reasoning: ReasoningDetails) -> ModelDetails {
        let mut details = ModelDetails::empty("openai/test-model".parse().unwrap());
        details.max_output_tokens = Some(128_000);
        details.reasoning = Some(reasoning);
        details
    }

    #[test]
    fn max_effort_is_sent_when_supported() {
        let details = model(ReasoningDetails::leveled(
            true, false, true, true, true, true, true,
        ));
        let config = convert_reasoning(
            CustomReasoningConfig {
                effort: ReasoningEffort::Max,
                exclude: false,
            },
            &details,
        );

        assert_eq!(config.effort, Some(types::ReasoningEffort::Max));
    }

    #[test]
    fn max_effort_degrades_to_xhigh_when_unsupported() {
        let details = model(ReasoningDetails::leveled(
            true, false, true, true, true, true, false,
        ));
        let config = convert_reasoning(
            CustomReasoningConfig {
                effort: ReasoningEffort::Max,
                exclude: false,
            },
            &details,
        );

        assert_eq!(config.effort, Some(types::ReasoningEffort::XHigh));
    }
}

mod parse_reasoning_mode {
    use openai_responses::types;

    use super::super::{REASONING_PRO_MODE, parse_reasoning_mode};
    use crate::model::ModelDetails;

    fn model(features: Vec<&'static str>) -> ModelDetails {
        let mut details = ModelDetails::empty("openai/test-model".parse().unwrap());
        details.features = features;
        details
    }

    #[test]
    fn pro_is_sent_when_supported() {
        assert_eq!(
            parse_reasoning_mode("pro", &model(vec![REASONING_PRO_MODE])),
            Some(types::ReasoningMode::Pro)
        );
    }

    #[test]
    fn pro_is_skipped_when_unsupported() {
        assert_eq!(parse_reasoning_mode("pro", &model(vec![])), None);
    }

    #[test]
    fn standard_and_unknown_values_are_ignored() {
        let details = model(vec![REASONING_PRO_MODE]);

        assert_eq!(parse_reasoning_mode("standard", &details), None);
        assert_eq!(parse_reasoning_mode("turbo", &details), None);
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

mod map_event {
    use openai_responses::types;
    use serde_json::json;

    use super::super::map_event;
    use crate::event::Event;

    fn collect(event: types::Event) -> Vec<Event> {
        map_event(event, false, true)
            .into_iter()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn function_call_added_emits_tool_call_start() {
        let events = collect(types::Event::OutputItemAdded {
            output_index: 2,
            item: serde_json::from_value(json!({
                "type": "function_call",
                "id": "fc_123",
                "status": "in_progress",
                "arguments": "",
                "call_id": "call_123",
                "name": "run_me"
            }))
            .unwrap(),
        });

        assert_eq!(events, vec![Event::tool_call_start(
            2, "call_123", "run_me"
        )]);
    }

    #[test]
    fn function_call_argument_delta_streams_as_tool_call_args() {
        let events = collect(types::Event::FunctionCallArgumentsDelta {
            delta: r#"{"path":"docs/rfd/drafts/D47"#.to_owned(),
            item_id: "fc_123".to_owned(),
            output_index: 2,
        });

        assert_eq!(events, vec![Event::tool_call_args(
            2,
            r#"{"path":"docs/rfd/drafts/D47"#
        )]);
    }

    #[test]
    fn function_call_done_emits_only_flush() {
        let events = collect(types::Event::OutputItemDone {
            output_index: 2,
            item: serde_json::from_value(json!({
                "type": "function_call",
                "id": "fc_123",
                "status": "completed",
                "arguments": "{\"path\":\"x\"}",
                "call_id": "call_123",
                "name": "run_me"
            }))
            .unwrap(),
        });

        assert_eq!(events, vec![Event::flush(2)]);
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

/// Tests recorded against the real OpenAI API.
///
/// Record with a valid `OPENAI_API_KEY`:
///
/// ```sh
/// RECORD=1 cargo test -p jp_llm recorded::
/// ```
///
/// Until the cassettes exist, these tests fail on replay.
mod recorded {
    use std::sync::Arc;

    use chrono::{TimeZone as _, Utc};
    use jp_conversation::{ConversationStream, event::ChatResponse};
    use jp_test::{Result, function_name};
    use test_log::test;

    use super::super::{ModelResponse, PROVIDER, map_model};
    use crate::{
        model::ModelDetails,
        test::{TestRequest, run_test},
    };

    /// Catalog `ModelDetails` for a real OpenAI model name, via `map_model` so
    /// the test cannot drift from the catalog entry.
    fn catalog_details(name: &str) -> ModelDetails {
        map_model(ModelResponse {
            id: name.to_owned(),
            _object: "model".to_owned(),
            _created: Utc.with_ymd_and_hms(2026, 4, 23, 0, 0, 0).unwrap(),
            _owned_by: "openai".to_owned(),
        })
        .unwrap()
    }

    /// Target the request at a real cataloged model: sets the config model id
    /// and replaces the test-default `ModelDetails` with the catalog entry.
    fn on_model(request: TestRequest, name: &str) -> TestRequest {
        let mut request = request.model(format!("openai/{name}").parse().unwrap());
        if let Some(details) = request.as_model_details_mut() {
            *details = catalog_details(name);
        }
        request
    }

    /// Set a catch-all parameter (`assistant.model.parameters.<key>`) on the
    /// request's base config.
    fn with_parameter(mut request: TestRequest, key: &str, value: &str) -> TestRequest {
        let Some(thread) = request.as_thread_mut() else {
            return request;
        };

        let mut base = (*thread.events.base_config()).clone();
        base.assistant
            .model
            .parameters
            .other
            .insert(key.to_owned(), serde_json::Value::from(value).into());

        let placeholder = ConversationStream::new(thread.events.base_config());
        let stream = std::mem::replace(&mut thread.events, placeholder);
        thread.events = stream.with_base_config(Arc::new(base));

        request
    }

    /// GPT-5.6 wire features against the real API: `reasoning.mode: "pro"`,
    /// `reasoning.context: "all_turns"`, explicit prompt-cache breakpoints, and
    /// `prompt_cache_key`.
    /// The second turn replays the first turn's reasoning items natively.
    ///
    /// The cassette pins the exact request bodies; the history assertion proves
    /// the model returned a native reasoning item (`openai_item_id`) that
    /// survives the round-trip.
    #[test(tokio::test)]
    async fn test_gpt_5_6_pro_reasoning_and_explicit_caching() -> Result {
        let first = with_parameter(
            on_model(
                TestRequest::chat(PROVIDER)
                    .enable_reasoning()
                    .chat_request("What is 7 * 191? Reason it through step by step."),
                "gpt-5.6",
            ),
            "reasoning_mode",
            "pro",
        );

        let second = with_parameter(
            on_model(
                TestRequest::chat(PROVIDER)
                    .enable_reasoning()
                    .chat_request("Repeat your previous answer."),
                "gpt-5.6",
            ),
            "reasoning_mode",
            "pro",
        )
        .assert_history(|history| {
            assert!(
                history.iter().any(|e| {
                    e.event
                        .as_chat_response()
                        .is_some_and(|r| matches!(r, ChatResponse::Reasoning { .. }))
                        && e.event.metadata.contains_key("openai_item_id")
                }),
                "expected a native reasoning item (openai_item_id) in history"
            );
        });

        run_test(PROVIDER, function_name!(), vec![first, second]).await
    }

    /// Whether the Responses API accepts native reasoning items when the target
    /// model is cataloged as reasoning-unsupported (a conversation switched
    /// from a reasoning model to gpt-4o mid-conversation).
    ///
    /// Replay is provenance-driven: a stored `openai_item_id` always replays as
    /// a native reasoning item, regardless of the target model's catalog entry.
    /// If recording fails with a 400 on the second request, the API rejects
    /// that shape and replay needs a known-unsupported fallback.
    #[test(tokio::test)]
    async fn test_reasoning_history_replayed_to_reasoning_unsupported_model() -> Result {
        // Turn 1 on the default reasoning test model produces native
        // reasoning items (openai_item_id + encrypted content) in history.
        let first = TestRequest::chat(PROVIDER)
            .enable_reasoning()
            .chat_request("What is 7 * 191?");

        // Turn 2 targets gpt-4o, whose catalog entry says
        // `ReasoningDetails::Unsupported`. The stored reasoning items are
        // still sent natively.
        let second = on_model(
            TestRequest::chat(PROVIDER).chat_request("Repeat your previous answer."),
            "gpt-4o",
        )
        .assert_history(|history| {
            let last_message = history
                .iter()
                .rev()
                .filter_map(|e| e.event.as_chat_response())
                .find_map(|r| match r {
                    ChatResponse::Message { message } => Some(message.clone()),
                    _ => None,
                });

            assert!(
                last_message.is_some_and(|m| !m.trim().is_empty()),
                "expected gpt-4o to answer after receiving replayed native reasoning items"
            );
        });

        run_test(PROVIDER, function_name!(), vec![first, second]).await
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
    /// hint.
    /// This used to surface as a non-retryable `Other` error.
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
    /// `type=service_unavailable_error, code=server_is_overloaded`.
    /// This used to surface as a non-retryable `Other` error because the
    /// message has no parseable retry-after hint.
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
