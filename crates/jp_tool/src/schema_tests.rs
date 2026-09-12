use serde_json::json;

use super::*;

mod validate {
    use super::*;

    fn validated(schema: &serde_json::Value) -> Result<(), Error> {
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

    /// An unresolvable reference is not the same as an absent `type`: what the
    /// node declares is unknown, not open, and forwarding a dangling pointer
    /// gets the whole request rejected by the provider.
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

    /// A property with no `type` keyword is valid JSON Schema meaning "any
    /// value", which is how a server declares a free-form parameter.
    #[test]
    fn a_property_with_no_type_is_unconstrained() {
        assert!(
            validated(&json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" },
                    "value": { "description": "Any JSON value." }
                }
            }))
            .is_ok()
        );
    }

    /// No declared type means no type for an enum value or a default to
    /// contradict.
    /// An `enum` still bounds the `default` it appears beside, which is why the
    /// two are declared on separate properties here.
    #[test]
    fn an_unconstrained_property_accepts_any_enum_and_default() {
        assert!(
            validated(&json!({
                "type": "object",
                "properties": {
                    "choice": { "enum": [1, "two", null, ["three"]] },
                    "value": { "default": { "a": 1 } }
                }
            }))
            .is_ok()
        );
    }

    /// `items` and `properties` apply only when the instance is an array or an
    /// object; neither needs a `type` to say so.
    #[test]
    fn an_unconstrained_property_may_carry_items_and_properties() {
        assert!(
            validated(&json!({
                "type": "object",
                "properties": {
                    "list": { "items": { "type": "string" } },
                    "target": { "properties": { "path": { "type": "string" } } }
                }
            }))
            .is_ok()
        );
    }

    /// A `properties` entry that is not a schema object declares nothing
    /// usable.
    /// JSON Schema's boolean form is legal, and `true` does mean "any value",
    /// but no other keyword can be read from it, so it is rejected alongside
    /// the shapes a schema-generation bug produces rather than forwarded to a
    /// provider that will reject the whole request.
    #[test]
    fn a_property_that_is_not_a_schema_object_is_rejected() {
        for value in [json!(null), json!("string"), json!(true), json!(false)] {
            assert_eq!(
                message_of(&json!({
                    "type": "object",
                    "properties": { "value": value }
                })),
                "Invalid schema at `tools.demo.parameters.value.type`: schema does not declare a \
                 supported type"
            );
        }
    }

    /// A `type` that is present but says nothing is malformed, not open.
    #[test]
    fn an_empty_type_list_is_rejected() {
        assert_eq!(
            message_of(&json!({
                "type": "object",
                "properties": { "value": { "type": [] } }
            })),
            "Invalid schema at `tools.demo.parameters.value.type`: schema does not declare a \
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

mod has_unconstrained_node {
    use super::*;

    #[test]
    fn finds_a_free_form_property_behind_a_reference() {
        assert!(has_unconstrained_node(&json!({
            "type": "object",
            "properties": { "payload": { "$ref": "#/$defs/Payload" } },
            "$defs": { "Payload": { "description": "Any JSON value." } }
        })));
    }

    #[test]
    fn reports_a_fully_typed_document() {
        assert!(!has_unconstrained_node(&json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array", "items": { "type": "string" } }
            }
        })));
    }

    /// The walk closes the same loop validation does, rather than expanding a
    /// self-referential type forever.
    #[test]
    fn a_recursive_schema_terminates() {
        assert!(!has_unconstrained_node(&json!({
            "type": "object",
            "properties": { "node": { "$ref": "#/$defs/Node" } },
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": { "child": { "$ref": "#/$defs/Node" } }
                }
            }
        })));
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
        assert!(kind.accepts_type(&json!("a")));
        assert!(!kind.accepts_type(&json!(1)));
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
    fn a_node_without_a_type_accepts_every_value() {
        let schema = json!({
            "type": "object",
            "properties": { "value": { "description": "Any JSON value." } }
        });

        let root = Node::root(&schema);
        let (_, value) = root.properties().into_iter().next().expect("property");

        assert!(value.is_unconstrained());
        assert!(value.types().is_empty());
        assert!(value.accepts_type(&json!("a")));
        assert!(value.accepts_type(&json!(1)));
        assert!(value.accepts_type(&json!(null)));
        assert!(value.accepts_type(&json!({ "a": 1 })));
    }

    /// Leaving the type open leaves the `enum` in charge: every value is of an
    /// acceptable type, and only the listed ones are permitted.
    #[test]
    fn an_enum_bounds_what_a_node_permits() {
        let schema = json!({
            "type": "object",
            "properties": { "value": { "enum": [3] } }
        });

        let root = Node::root(&schema);
        let (_, value) = root.properties().into_iter().next().expect("property");

        assert!(value.accepts_type(&json!("3")));
        assert!(!value.permits(&json!("3")));
        assert!(value.permits(&json!(3)));
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
