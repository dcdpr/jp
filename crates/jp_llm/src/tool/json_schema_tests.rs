use indexmap::IndexMap;
use jp_config::conversation::tool::ToolParameterConfig;
use jp_tool::schema::validate;
use serde_json::json;

use super::*;

/// Parse a parameter override the way a configuration file would produce it.
fn config(value: serde_json::Value) -> ToolParameterConfig {
    serde_json::from_value(value).expect("valid parameter config")
}

fn configs(values: &[(&str, serde_json::Value)]) -> IndexMap<String, ToolParameterConfig> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), config(value.clone())))
        .collect()
}

fn error_of(result: Result<Value, Error>) -> String {
    result.unwrap_err().to_string()
}

mod from_config {
    use super::*;

    #[test]
    fn builds_an_object_schema() {
        let parameters = configs(&[
            ("path", json!({ "type": "string", "required": true })),
            (
                "limit",
                json!({ "type": "integer", "default": 10, "summary": "How many." }),
            ),
        ]);

        let schema = from_config("tools.demo.parameters", &parameters).unwrap();

        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "default": 10, "description": "How many." }
                },
                "required": ["path"]
            })
        );
    }

    #[test]
    fn builds_nested_arrays_and_objects() {
        let parameters = configs(&[
            (
                "tags",
                json!({ "type": "array", "items": { "type": "string", "enum": ["a", "b"] } }),
            ),
            (
                "target",
                json!({
                    "type": "object",
                    "properties": { "path": { "type": "string", "required": true } }
                }),
            ),
        ]);

        let schema = from_config("tools.demo.parameters", &parameters).unwrap();

        assert_eq!(
            schema,
            json!({
                "type": "object",
                "properties": {
                    "tags": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["a", "b"] }
                    },
                    "target": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }
                },
                "required": []
            })
        );
    }

    #[test]
    fn a_parameter_without_a_type_is_rejected() {
        let parameters = configs(&[("path", json!({ "summary": "Where." }))]);

        assert_eq!(
            error_of(from_config("tools.demo.parameters", &parameters)),
            "Invalid schema at `tools.demo.parameters.path.type`: local and built-in tool \
             parameters must declare a type"
        );
    }
}

mod with_overrides {
    use super::*;

    /// The server's document is the source of truth: anything the override does
    /// not speak to survives untouched, `$defs` included.
    #[test]
    fn preserves_the_server_document() {
        let source = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "title": "CreateNote",
            "properties": {
                "title": { "type": "string" },
                "tags": { "type": "array", "items": { "$ref": "#/$defs/Tag" } }
            },
            "required": ["title"],
            "$defs": {
                "Tag": { "type": "string" }
            }
        });
        let overrides = configs(&[("tags", json!({ "items": { "enum": ["task", "idea"] } }))]);

        let schema = with_overrides("tools.notes.parameters", &source, &overrides).unwrap();

        assert_eq!(
            schema,
            json!({
                "$schema": "https://json-schema.org/draft/2020-12/schema",
                "type": "object",
                "title": "CreateNote",
                "properties": {
                    "title": { "type": "string" },
                    "tags": {
                        "type": "array",
                        "items": { "$ref": "#/$defs/Tag", "enum": ["task", "idea"] }
                    }
                },
                "required": ["title"],
                "$defs": {
                    "Tag": { "type": "string" }
                }
            })
        );
    }

    /// The reference stays a reference.
    /// Narrowing it adds a sibling keyword rather than expanding the definition
    /// into the document.
    #[test]
    fn a_referenced_item_keeps_its_reference() {
        let source = json!({
            "type": "object",
            "properties": {
                "kinds": { "type": "array", "items": { "$ref": "#/$defs/EntryType" } }
            },
            "$defs": { "EntryType": { "type": "string", "enum": ["Enum", "Method"] } }
        });
        let overrides = configs(&[("kinds", json!({ "items": { "type": "string" } }))]);

        let schema = with_overrides("tools.docs.parameters", &source, &overrides).unwrap();

        assert_eq!(
            schema["properties"]["kinds"]["items"],
            json!({ "$ref": "#/$defs/EntryType" })
        );
    }

    #[test]
    fn an_empty_enum_clears_an_inherited_one() {
        let source = json!({
            "type": "object",
            "properties": { "state": { "type": "string", "enum": ["open", "closed"] } }
        });
        let overrides = configs(&[("state", json!({ "enum": [] }))]);

        let schema = with_overrides("tools.demo.parameters", &source, &overrides).unwrap();

        assert_eq!(schema["properties"]["state"], json!({ "type": "string" }));
    }

    #[test]
    fn a_contradicting_type_is_rejected() {
        let source = json!({
            "type": "object",
            "properties": { "count": { "type": "integer" } }
        });
        let overrides = configs(&[("count", json!({ "type": "string" }))]);

        assert_eq!(
            error_of(with_overrides("tools.demo.parameters", &source, &overrides)),
            "Invalid schema at `tools.demo.parameters.count.type`: MCP declares integer, but the \
             configuration declares string"
        );
    }

    /// A referenced type is compared through the document, so restating it
    /// correctly is accepted and restating it wrongly is not.
    #[test]
    fn a_contradicting_type_is_rejected_through_a_reference() {
        let source = json!({
            "type": "object",
            "properties": { "kind": { "$ref": "#/$defs/Kind" } },
            "$defs": { "Kind": { "type": "string" } }
        });
        let overrides = configs(&[("kind", json!({ "type": "integer" }))]);

        assert_eq!(
            error_of(with_overrides("tools.demo.parameters", &source, &overrides)),
            "Invalid schema at `tools.demo.parameters.kind.type`: MCP declares string, but the \
             configuration declares integer"
        );
    }

    /// JSON Schema type arrays are unordered, and a single-element array means
    /// the same as the bare string.
    #[test]
    fn a_matching_type_may_be_restated_in_any_form() {
        let source = json!({
            "type": "object",
            "properties": {
                "content": { "type": ["string", "null"] },
                "name": { "type": "string" }
            }
        });
        let overrides = configs(&[
            ("content", json!({ "type": ["null", "string"] })),
            ("name", json!({ "type": ["string"] })),
        ]);

        let schema = with_overrides("tools.demo.parameters", &source, &overrides).unwrap();

        assert_eq!(
            schema["properties"]["content"]["type"],
            json!(["string", "null"])
        );
        assert_eq!(schema["properties"]["name"]["type"], json!("string"));
    }

    #[test]
    fn required_can_be_tightened_but_not_loosened() {
        let source = json!({
            "type": "object",
            "properties": { "a": { "type": "string" }, "b": { "type": "string" } },
            "required": ["b"]
        });
        let overrides = configs(&[
            ("a", json!({ "required": true })),
            ("b", json!({ "required": false })),
        ]);

        let schema = with_overrides("tools.demo.parameters", &source, &overrides).unwrap();

        assert_eq!(schema["required"], json!(["b", "a"]));
    }

    /// A property with no `type` is the server saying "any value".
    /// The document keeps it as written and the tool stays usable.
    #[test]
    fn a_free_form_property_survives_and_validates() {
        let source = json!({
            "type": "object",
            "properties": {
                "key": { "type": "string" },
                "value": { "description": "Any JSON value." }
            }
        });

        let schema = with_overrides("tools.store.parameters", &source, &IndexMap::new()).unwrap();

        assert_eq!(
            schema["properties"]["value"],
            json!({ "description": "Any JSON value." })
        );
        assert!(validate("tools.store.parameters", &schema).is_ok());
    }

    /// Nothing was declared, so nothing is contradicted: configuration may
    /// narrow a free-form property to the shape the user actually wants.
    #[test]
    fn a_free_form_property_can_be_narrowed_by_configuration() {
        let source = json!({
            "type": "object",
            "properties": { "value": { "description": "Any JSON value." } }
        });
        let overrides = configs(&[("value", json!({ "type": "object" }))]);

        let schema = with_overrides("tools.store.parameters", &source, &overrides).unwrap();

        assert_eq!(
            schema["properties"]["value"],
            json!({ "type": "object", "description": "Any JSON value." })
        );
    }

    #[test]
    fn a_property_the_server_omits_is_added() {
        let source = json!({ "type": "object", "properties": {} });
        let overrides = configs(&[("extra", json!({ "type": "string", "summary": "Added." }))]);

        let schema = with_overrides("tools.demo.parameters", &source, &overrides).unwrap();

        assert_eq!(
            schema["properties"]["extra"],
            json!({ "type": "string", "description": "Added." })
        );
    }
}
