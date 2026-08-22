use indexmap::IndexMap;
use jp_config::conversation::tool::ToolParameterConfig;
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

fn error_of(result: Result<Value, ToolError>) -> String {
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

mod validate {
    use super::*;

    fn validated(schema: &serde_json::Value) -> Result<(), ToolError> {
        validate("tools.demo.parameters", schema)
    }

    fn message_of(schema: &serde_json::Value) -> String {
        validated(schema).unwrap_err().to_string()
    }

    #[test]
    fn an_array_must_declare_items() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": { "tags": { "type": "array" } }
            })),
            "Invalid schema at `tools.demo.parameters.tags.items`: array schemas must declare an \
             item schema"
        );
    }

    #[test]
    fn items_require_an_array_type() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": { "tags": { "type": "string", "items": { "type": "string" } } }
            })),
            "Invalid schema at `tools.demo.parameters.tags.items`: `items` requires an array \
             type, but the schema requires string"
        );
    }

    #[test]
    fn properties_require_an_object_type() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "properties": { "a": { "type": "string" } } }
                }
            })),
            "Invalid schema at `tools.demo.parameters.target.properties`: `properties` requires \
             an object type, but the schema requires string"
        );
    }

    #[test]
    fn a_scalar_enum_on_an_array_points_at_items() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": {
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "enum": ["projects/jp"]
                    }
                }
            })),
            "Invalid schema at `tools.demo.parameters.tags.enum`: enum value \"projects/jp\" has \
             type string, but the schema requires array; use \
             `tools.demo.parameters.tags.items.enum` to constrain array elements"
        );
    }

    #[test]
    fn enum_values_must_be_unique() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": { "kind": { "type": "string", "enum": ["task", "task"] } }
            })),
            "Invalid schema at `tools.demo.parameters.kind.enum`: enum values must be unique; \
             duplicate value \"task\""
        );
    }

    #[test]
    fn a_default_outside_the_enum_is_rejected() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": {
                    "state": { "type": "string", "enum": ["open"], "default": "all" }
                }
            })),
            "Invalid schema at `tools.demo.parameters.state.default`: default value \"all\" is \
             not allowed by the enum"
        );
    }

    #[test]
    fn a_default_must_match_the_item_schema() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": {
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "default": ["task", 1]
                    }
                }
            })),
            "Invalid schema at `tools.demo.parameters.tags.default[1]`: default value 1 has type \
             integer, but the schema requires string"
        );
    }

    #[test]
    fn a_default_must_match_a_property_enum() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "object",
                        "properties": { "mode": { "type": "string", "enum": ["safe"] } },
                        "default": { "mode": "fast" }
                    }
                }
            })),
            "Invalid schema at `tools.demo.parameters.target.default.mode`: default value \
             \"fast\" is not allowed by the enum"
        );
    }

    #[test]
    fn an_unsupported_type_is_rejected() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": { "name": { "type": "strng" } }
            })),
            "Invalid schema at `tools.demo.parameters.name.type`: unsupported JSON type `strng`"
        );
    }

    #[test]
    fn duplicate_types_are_rejected() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": { "name": { "type": ["string", "string"] } }
            })),
            "Invalid schema at `tools.demo.parameters.name.type`: type values must be unique; \
             duplicate type `string`"
        );
    }

    #[test]
    fn a_node_without_a_usable_type_is_rejected() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": { "thing": { "$ref": "https://example.com/schema.json#/Thing" } }
            })),
            "Invalid schema at `tools.demo.parameters.thing.type`: schema does not declare a \
             supported type"
        );
    }

    /// Validation reads through references, so a constraint behind a `$ref` is
    /// enforced exactly as an inline one would be.
    #[test]
    fn constraints_behind_a_reference_are_enforced() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": {
                    "kind": { "$ref": "#/$defs/Kind", "default": "fast" }
                },
                "$defs": { "Kind": { "type": "string", "enum": ["safe"] } }
            })),
            "Invalid schema at `tools.demo.parameters.kind.default`: default value \"fast\" is \
             not allowed by the enum"
        );
    }

    /// A self-referential type is legal.
    /// Providers that reject recursion say so themselves; validation must
    /// terminate rather than expand forever.
    #[test]
    fn a_recursive_schema_is_accepted() {
        assert!(
            validated(&json!({
                "type": "object",
                "properties": { "node": { "$ref": "#/$defs/Node" } },
                "$defs": {
                    "Node": {
                        "type": "object",
                        "properties": {
                            "value": { "type": "string" },
                            "child": { "$ref": "#/$defs/Node" }
                        }
                    }
                }
            }))
            .is_ok()
        );
    }

    /// Mutually recursive definitions close the same loop through two pointers.
    #[test]
    fn mutually_recursive_definitions_are_accepted() {
        assert!(
            validated(&json!({
                "type": "object",
                "properties": { "a": { "$ref": "#/$defs/A" } },
                "$defs": {
                    "A": { "type": "object", "properties": { "b": { "$ref": "#/$defs/B" } } },
                    "B": { "type": "object", "properties": { "a": { "$ref": "#/$defs/A" } } }
                }
            }))
            .is_ok()
        );
    }
}

mod inline {
    use super::*;

    #[test]
    fn expands_references_and_drops_definitions() {
        let expanded = inline(&json!({
            "type": "object",
            "properties": {
                "kinds": { "type": "array", "items": { "$ref": "#/$defs/EntryType" } }
            },
            "$defs": { "EntryType": { "type": "string", "enum": ["Enum"] } }
        }));

        assert_eq!(
            expanded,
            json!({
                "type": "object",
                "properties": {
                    "kinds": {
                        "type": "array",
                        "items": { "type": "string", "enum": ["Enum"] }
                    }
                }
            })
        );
    }

    #[test]
    fn sibling_keys_win_over_the_definition() {
        let expanded = inline(&json!({
            "type": "object",
            "properties": {
                "mode": { "$ref": "#/$defs/Mode", "description": "from the parameter" }
            },
            "$defs": { "Mode": { "type": "string", "description": "from defs" } }
        }));

        assert_eq!(
            expanded["properties"]["mode"],
            json!({ "type": "string", "description": "from the parameter" })
        );
    }

    /// A recursive type has no finite expansion, so the innermost reference is
    /// left as written rather than looping.
    #[test]
    fn a_recursive_reference_terminates() {
        let expanded = inline(&json!({
            "type": "object",
            "properties": { "node": { "$ref": "#/$defs/Node" } },
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": { "child": { "$ref": "#/$defs/Node" } }
                }
            }
        }));

        assert_eq!(
            expanded["properties"]["node"],
            json!({
                "type": "object",
                "properties": { "child": { "$ref": "#/$defs/Node" } }
            })
        );
    }

    #[test]
    fn an_unresolvable_reference_is_left_in_place() {
        let expanded = inline(&json!({
            "type": "object",
            "properties": { "thing": { "$ref": "https://example.com/s.json#/Thing" } }
        }));

        assert_eq!(
            expanded["properties"]["thing"],
            json!({ "$ref": "https://example.com/s.json#/Thing" })
        );
    }
}

mod node {
    use super::*;

    #[test]
    fn reads_through_a_reference() {
        let schema = json!({
            "type": "object",
            "properties": { "kind": { "$ref": "#/$defs/Kind" } },
            "$defs": { "Kind": { "type": "string", "enum": ["a", "b"] } }
        });

        let root = Node::root(&schema);
        let (_, kind) = root
            .properties()
            .into_iter()
            .find(|(name, _)| name == "kind")
            .expect("property");

        assert_eq!(kind.types(), vec!["string".to_owned()]);
        assert_eq!(kind.enumeration(), vec![json!("a"), json!("b")]);
        assert!(kind.accepts(&json!("a")));
        assert!(!kind.accepts(&json!(1)));
    }

    /// Sibling keys win over the referenced definition, per JSON Schema
    /// 2020-12.
    #[test]
    fn sibling_keys_win_over_the_definition() {
        let schema = json!({
            "type": "object",
            "properties": {
                "kind": { "$ref": "#/$defs/Kind", "description": "from the parameter" }
            },
            "$defs": { "Kind": { "type": "string", "description": "from defs" } }
        });

        let root = Node::root(&schema);
        let (_, kind) = root.properties().into_iter().next().expect("property");

        assert_eq!(kind.origin(), Some("#/$defs/Kind"));
        assert_eq!(kind.types(), vec!["string".to_owned()]);
    }

    #[test]
    fn reports_required_properties() {
        let schema = json!({
            "type": "object",
            "properties": { "a": { "type": "string" }, "b": { "type": "string" } },
            "required": ["a"]
        });

        let root = Node::root(&schema);

        assert!(root.is_required("a"));
        assert!(!root.is_required("b"));
    }
}
