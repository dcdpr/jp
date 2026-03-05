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
