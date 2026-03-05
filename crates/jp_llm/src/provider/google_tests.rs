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
        .model("google/gemini-3-pro-preview".parse().unwrap())
        .event(ChatRequest::from("Test message"));

    run_test(PROVIDER, function_name!(), Some(request)).await
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
            "required": ["inquiry_id", "answer"]
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
            "items": { "$ref": "#/$defs/CountryInfo" },
            "$defs": {
                "CountryInfo": {
                    "type": "object",
                    "properties": {
                        "continent": { "type": "string" },
                        "gdp": { "type": "integer" }
                    },
                    "required": ["continent", "gdp"]
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
                        "name": { "type": "string" }
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
                "addr": { "$ref": "#/$defs/Address" }
            },
            "$defs": {
                "Address": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" },
                        "zip": { "type": "string" }
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
                "x": { "$ref": "#/$defs/X" }
            },
            "definitions": {
                "X": { "type": "string" }
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
                "first": { "type": "string" },
                "second": { "type": "integer" },
                "third": { "type": "boolean" }
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
                "only": { "type": "string" }
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
                "a": { "type": "string" },
                "b": { "type": "string" }
            },
            "propertyOrdering": ["b", "a"]
        }));

        let out = transform_schema(input);

        // Existing ordering should not be overwritten.
        assert_eq!(out["propertyOrdering"], json!(["b", "a"]));
    }

    #[test]
    fn anyof_variants_processed() {
        let input = schema(json!({
            "anyOf": [
                { "type": "string", "const": "fixed" },
                { "type": "integer" }
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
                { "$ref": "#/$defs/Str" },
                { "type": "integer" }
            ],
            "$defs": {
                "Str": { "type": "string" }
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
                "name": { "type": "string" }
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
                "name": { "type": "string" }
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
                { "type": "string", "const": "header" },
                { "type": "integer" }
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
            "enum": ["A", "B", "C"]
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
            "required": ["inquiry_id", "answer"],
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
            "items": { "$ref": "#/$defs/CountryInfo" },
            "title": "Placeholder",
            "type": "array",
            "$defs": {
                "CountryInfo": {
                    "properties": {
                        "continent": { "title": "Continent", "type": "string" },
                        "gdp": { "title": "Gdp", "type": "integer" }
                    },
                    "required": ["continent", "gdp"],
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
