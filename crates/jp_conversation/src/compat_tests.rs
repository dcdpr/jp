use jp_config::{
    AppConfig, PartialAppConfig, Schema,
    schema::{BooleanType, StructType},
};
use serde_json::json;

use super::*;

#[test]
fn strip_noop_when_all_fields_known() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "style": {
            "code": {
                "color": true
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 0);
    assert_eq!(
        value,
        json!({
            "style": {
                "code": {
                    "color": true
                }
            }
        })
    );
}

#[test]
fn strip_removes_unknown_top_level_field() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "style": {},
        "this_field_does_not_exist": 42
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(value, json!({ "style": {} }));
}

#[test]
fn strip_removes_unknown_nested_field() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "style": {
            "code": {
                "color": true,
                "theme": "dracula"
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value,
        json!({
            "style": {
                "code": {
                    "color": true
                }
            }
        })
    );
}

#[test]
fn strip_removes_multiple_unknown_fields_at_different_levels() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "gone_top": true,
        "style": {
            "gone_mid": "bye",
            "code": {
                "color": true,
                "gone_leaf": 99
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 3);
    assert_eq!(
        value,
        json!({
            "style": {
                "code": {
                    "color": true
                }
            }
        })
    );
}

#[test]
fn strip_leaves_non_object_values_untouched() {
    let schema = AppConfig::schema();
    let mut value = json!("just a string");

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 0);
    assert_eq!(value, json!("just a string"));
}

#[test]
fn strip_empty_object_is_noop() {
    let schema = AppConfig::schema();
    let mut value = json!({});

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 0);
}

#[test]
fn strip_removes_entire_unknown_nested_section() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "removed_section": {
            "a": 1,
            "b": { "c": 2 }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(value, json!({}));
}

#[test]
fn strip_with_minimal_synthetic_schema() {
    // Verify the function works with a hand-built schema, independent of
    // AppConfig. This protects against future SchemaBuilder changes.
    let schema = Schema::structure(StructType::new([(
        "keep".to_owned(),
        Schema::boolean(BooleanType::default()),
    )]));

    let mut value = json!({
        "keep": true,
        "drop": "gone"
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(value, json!({ "keep": true }));
}

#[test]
fn strip_descends_into_flattened_map_entries() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "bash": {
                    "source": "local",
                    "access": {
                        "fs": [{ "path": ".", "read": true }],
                        "from_a_newer_jp": [{ "host": "example.com" }]
                    }
                }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value,
        json!({
            "conversation": {
                "tools": {
                    "bash": {
                        "source": "local",
                        "access": {
                            "fs": [{ "path": ".", "read": true }]
                        }
                    }
                }
            }
        })
    );
}

#[test]
fn strip_keeps_tool_names_at_the_flattened_level() {
    // Every key under `conversation.tools` other than `*` is a tool name, not a
    // field, so none of them may be treated as unknown.
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "*": { "run": "ask" },
                "a_tool_this_binary_never_heard_of": { "source": "local" }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 0);
    assert_eq!(
        value,
        json!({
            "conversation": {
                "tools": {
                    "*": { "run": "ask" },
                    "a_tool_this_binary_never_heard_of": { "source": "local" }
                }
            }
        })
    );
}

#[test]
fn strip_descends_into_a_tool_named_after_the_flattened_field() {
    // `ToolsConfig` flattens its per-tool map, so the map's own Rust field name
    // is not a key in the serialized form. A tool that happens to share that
    // name is an entry like any other.
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "tools": { "source": "local", "from_a_newer_jp": 1 }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value,
        json!({
            "conversation": {
                "tools": {
                    "tools": { "source": "local" }
                }
            }
        })
    );
}

#[test]
fn strip_descends_into_array_items() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "assistant": {
            "instructions": [{ "title": "t", "from_a_newer_jp": 1 }]
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value,
        json!({ "assistant": { "instructions": [{ "title": "t" }] } })
    );
}

#[test]
fn strip_descends_into_mergeable_vec_items() {
    // A vector field declared with `partial_via = MergeableVec` also reaches
    // disk as `{ "value": [...], "strategy": ... }`, which is the shape
    // `ConversationStream::to_parts` writes.
    let schema = AppConfig::schema();
    let mut value = json!({
        "assistant": {
            "instructions": {
                "value": [{ "title": "t", "from_a_newer_jp": 1 }],
                "strategy": "replace"
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value,
        json!({
            "assistant": {
                "instructions": {
                    "value": [{ "title": "t" }],
                    "strategy": "replace"
                }
            }
        })
    );
}

#[test]
fn strip_descends_into_nested_maps() {
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "bash": {
                    "parameters": {
                        "cmd": { "type": "string", "from_a_newer_jp": 1 }
                    }
                }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value,
        json!({
            "conversation": {
                "tools": {
                    "bash": { "parameters": { "cmd": { "type": "string" } } }
                }
            }
        })
    );
}

#[test]
fn strip_resolves_a_reference_to_the_type_it_names() {
    // `ToolParameterConfig` contains itself through `items`, so the schema
    // builder emits a reference there rather than expanding forever.
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "fs_modify_file": {
                    "parameters": {
                        "patterns": {
                            "type": "array",
                            "items": { "type": "string", "from_a_newer_jp": 1 }
                        }
                    }
                }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value,
        json!({
            "conversation": {
                "tools": {
                    "fs_modify_file": {
                        "parameters": {
                            "patterns": {
                                "type": "array",
                                "items": { "type": "string" }
                            }
                        }
                    }
                }
            }
        })
    );
}

#[test]
fn strip_resolves_a_reference_reached_through_a_map() {
    // `properties` is the other recursive position: a map whose values are
    // parameters again.
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "fs_modify_file": {
                    "parameters": {
                        "patch": {
                            "type": "object",
                            "properties": {
                                "old": { "type": "string", "from_a_newer_jp": 1 }
                            }
                        }
                    }
                }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value["conversation"]["tools"]["fs_modify_file"]["parameters"]["patch"]["properties"]
            ["old"],
        json!({ "type": "string" })
    );
}

#[test]
fn strip_resolves_a_reference_at_every_depth_it_recurses() {
    // Resolving once is not enough: the reference the resolved schema itself
    // carries has to resolve too, however deep the stored value goes.
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "fs_modify_file": {
                    "parameters": {
                        "patterns": {
                            "type": "array",
                            "items": {
                                "type": "array",
                                "items": { "type": "string", "from_a_newer_jp": 1 }
                            }
                        }
                    }
                }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value["conversation"]["tools"]["fs_modify_file"]["parameters"]["patterns"]["items"]
            ["items"],
        json!({ "type": "string" })
    );
}

#[test]
fn strip_descends_into_the_one_union_variant_that_fits() {
    // `enable` accepts a bool, a legacy string, or a `{ state, allow_toggle }`
    // table. A table can only be the third, and unlike the untagged unions
    // beside it, its hand-written `Deserialize` rejects unknown keys.
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "bash": {
                    "enable": { "state": true, "from_a_newer_jp": 1 },
                    "command": { "program": "bash", "from_a_newer_jp": 2 }
                }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 2);
    assert_eq!(
        value,
        json!({
            "conversation": {
                "tools": {
                    "bash": {
                        "enable": { "state": true },
                        "command": { "program": "bash" }
                    }
                }
            }
        })
    );
}

#[test]
fn strip_descends_into_a_per_question_instructions_wrapper() {
    // `questions.<id>.target` is the one place a *partial* type's schema enters
    // the tree, and every field of a partial is nullified, so `instructions` is
    // a union of its array and null rather than a bare array. The stored value
    // is an object, which matches neither, so nothing below it can be reached
    // unless the union is unwrapped first.
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "fs_modify_file": {
                    "questions": {
                        "apply_changes": {
                            "target": {
                                "instructions": {
                                    "value": [{ "title": "Review", "from_a_newer_jp": 1 }],
                                    "strategy": "replace"
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 1);
    assert_eq!(
        value["conversation"]["tools"]["fs_modify_file"]["questions"]["apply_changes"]["target"]
            ["instructions"]["value"][0],
        json!({ "title": "Review" })
    );
}

#[test]
fn a_field_added_to_a_per_question_instruction_does_not_wipe_the_config() {
    let value = json!({
        "style": { "code": { "color": false } },
        "conversation": {
            "tools": {
                "fs_modify_file": {
                    "questions": {
                        "apply_changes": {
                            "target": {
                                "instructions": {
                                    "value": [{ "title": "Review", "from_a_newer_jp": 1 }],
                                    "strategy": "replace"
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert_eq!(
        config.style.code.color,
        Some(false),
        "a setting elsewhere in the config must survive the unreadable instruction field"
    );
}

#[test]
fn strip_leaves_a_union_alone_when_two_variants_fit() {
    use jp_config::schema::UnionType;

    let one = Schema::structure(StructType::new([(
        "a".to_owned(),
        Schema::boolean(BooleanType::default()),
    )]));
    let other = Schema::structure(StructType::new([(
        "b".to_owned(),
        Schema::boolean(BooleanType::default()),
    )]));
    let schema = Schema::union(UnionType::new_any([one, other]));

    let mut value = json!({ "a": true, "b": false });
    let stripped = strip_unknown_fields(&mut value, &schema);

    assert_eq!(stripped, 0, "neither variant may claim the value");
    assert_eq!(value, json!({ "a": true, "b": false }));
}

#[test]
fn strip_leaves_free_form_tool_options_alone() {
    // `options` is a free-form JSON map whose contents this binary cannot
    // describe, so nothing in it is "unknown".
    let schema = AppConfig::schema();
    let mut value = json!({
        "conversation": {
            "tools": {
                "bash": { "options": { "anything": { "nested": [1, 2, 3] } } }
            }
        }
    });

    let stripped = strip_unknown_fields(&mut value, &schema);
    assert_eq!(stripped, 0);
    assert_eq!(
        value,
        json!({
            "conversation": {
                "tools": {
                    "bash": { "options": { "anything": { "nested": [1, 2, 3] } } }
                }
            }
        })
    );
}

#[test]
fn a_field_added_under_a_tool_does_not_wipe_the_stored_config() {
    // The shape that a newer JP writes and an older one has to survive: a field
    // it has no type for, nested under the flattened per-tool map. Losing the
    // whole config here costs the model id, which no schematic default
    // supplies, and the conversation stops loading altogether.
    let value = json!({
        "assistant": { "model": { "id": "anthropic/claude-sonnet-4" } },
        "conversation": {
            "tools": {
                "bash": {
                    "source": "local",
                    "access": { "from_a_newer_jp": [{ "host": "example.com" }] }
                }
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert_eq!(
        config.assistant.model.id.to_string(),
        "anthropic/claude-sonnet-4"
    );
    assert_eq!(
        config.conversation.tools.tools["bash"].source,
        Some(jp_config::conversation::tool::ToolSource::Local { tool: None })
    );
}

#[test]
fn schema_top_level_is_struct_with_style() {
    let schema = AppConfig::schema();
    let jp_config::SchemaType::Struct(ref s) = schema.ty else {
        panic!("top-level schema is not a struct: {:?}", schema.ty);
    };
    assert!(s.fields.contains_key("style"), "missing 'style' field");
    assert!(
        s.fields.contains_key("assistant"),
        "missing 'assistant' field"
    );
}

#[test]
fn schema_tools_has_flattened_field() {
    let schema = AppConfig::schema();
    let jp_config::SchemaType::Struct(ref top) = schema.ty else {
        panic!("top-level not struct");
    };

    let conv = top
        .fields
        .get("conversation")
        .expect("missing 'conversation'");
    let jp_config::SchemaType::Struct(ref conv_s) = conv.schema.ty else {
        panic!("conversation not struct");
    };

    let tools = conv_s.fields.get("tools").expect("missing 'tools'");
    let jp_config::SchemaType::Struct(ref tools_s) = tools.schema.ty else {
        panic!("tools not struct: {:?}", tools.schema.ty);
    };

    // The `*` (defaults) field should exist
    assert!(tools_s.fields.contains_key("*"), "missing '*' field");

    // At least one field should be flattened (the tools IndexMap)
    let has_flatten = tools_s.fields.values().any(|f| f.flatten);
    assert!(
        has_flatten,
        "expected a flattened field in ToolsConfig schema"
    );
}

#[test]
fn schema_style_code_is_struct_with_color() {
    let schema = AppConfig::schema();
    let jp_config::SchemaType::Struct(ref top) = schema.ty else {
        panic!("top-level not struct");
    };

    let style_field = top.fields.get("style").expect("missing 'style'");
    let jp_config::SchemaType::Struct(ref style) = style_field.schema.ty else {
        panic!("style is not a struct: {:?}", style_field.schema.ty);
    };

    let code_field = style.fields.get("code").expect("missing 'code'");
    let jp_config::SchemaType::Struct(ref code) = code_field.schema.ty else {
        panic!("code is not a struct: {:?}", code_field.schema.ty);
    };

    assert!(code.fields.contains_key("color"), "missing 'color'");
    assert!(
        code.fields.contains_key("line_numbers"),
        "missing 'line_numbers'"
    );
    assert!(
        !code.fields.contains_key("removed_field"),
        "should not have 'removed_field'"
    );
}

/// Provider parameters survive a stored config, in either spelling.
///
/// They are collected into a flattened field, so they arrive as keys the schema
/// does not name.
/// Stripping would forward the conversation's next request without them,
/// silently changing what the model is asked.
#[test]
fn partial_config_keeps_provider_parameters() {
    let value = json!({
        "assistant": {
            "model": {
                "parameters": {
                    "temperature": 0.7,
                    "presence_penalty": 0.5,
                },
            },
        },
    });

    let config = deserialize_partial_config(value);
    let parameters = &config.assistant.model.parameters;

    assert_eq!(parameters.temperature, Some(0.7));
    assert_eq!(
        parameters
            .other
            .as_ref()
            .and_then(|o| o.get("presence_penalty")),
        Some(&jp_config::types::json_value::JsonValue(json!(0.5))),
        "a parameter JP does not model is not a stray key to strip"
    );
}

/// A config stored before the collector was flattened nested its parameters
/// under `other`, and they still arrive as parameters.
#[test]
fn partial_config_hoists_a_legacy_other_table() {
    let value = json!({
        "assistant": {
            "model": {
                "parameters": {
                    "other": { "presence_penalty": 0.5 },
                },
            },
        },
    });

    let config = deserialize_partial_config(value);
    let other = config
        .assistant
        .model
        .parameters
        .other
        .as_ref()
        .expect("the legacy table is hoisted");

    assert_eq!(
        other.get("presence_penalty"),
        Some(&jp_config::types::json_value::JsonValue(json!(0.5)))
    );
    assert!(
        !other.contains_key("other"),
        "the wrapper is not itself a parameter: {other:?}"
    );
}

#[test]
fn strip_directly_on_delta_subtree() {
    // Reproduce exactly what deserialize_config_delta does: strip the "delta"
    // sub-value, not the whole event JSON.
    let schema = AppConfig::schema();
    let mut delta_value = json!({
        "style": {
            "code": {
                "color": false,
                "removed_field": "stale"
            }
        }
    });

    let stripped = strip_unknown_fields(&mut delta_value, &schema);
    assert_eq!(stripped, 1, "should have stripped 'removed_field'");
    assert_eq!(
        delta_value,
        json!({ "style": { "code": { "color": false } } })
    );
}

#[test]
fn partial_config_strips_unknown_and_preserves_known() {
    let value = json!({
        "style": {
            "code": {
                "color": false,
                "removed_field": "stale"
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert_eq!(
        config.style.code.color,
        Some(false),
        "known field 'color' should survive"
    );
}

#[test]
fn partial_config_valid() {
    let value = json!({
        "style": {
            "code": {
                "color": false
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert_eq!(config.style.code.color, Some(false));
}

#[test]
fn partial_config_strips_unknown_preserves_known() {
    let value = json!({
        "style": {
            "code": {
                "color": true,
                "theme": "dracula"
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert_eq!(config.style.code.color, Some(true));
}

#[test]
fn a_type_change_drops_only_the_field_that_changed() {
    // `color` expects a bool. Stripping cannot help — the key is known, it is
    // the value that this binary has no type for — so the field is dropped by
    // path and everything around it is kept.
    let value = json!({
        "assistant": { "model": { "id": "anthropic/claude-sonnet-4" } },
        "style": {
            "code": {
                "color": [1, 2, 3],
                "line_numbers": true
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert!(config.style.code.color.is_none());
    assert_eq!(config.style.code.line_numbers, Some(true));
    assert_eq!(
        config.assistant.model.id.to_string(),
        "anthropic/claude-sonnet-4"
    );
}

#[test]
fn a_type_change_inside_an_array_drops_only_that_element_field() {
    let value = json!({
        "assistant": {
            "model": { "id": "anthropic/claude-sonnet-4" },
            "instructions": [{ "title": "kept", "position": { "was": "a number" } }]
        }
    });

    let config = deserialize_partial_config(value);
    assert_eq!(
        config.assistant.model.id.to_string(),
        "anthropic/claude-sonnet-4"
    );
    assert_eq!(config.assistant.instructions.len(), 1);
    assert_eq!(
        config.assistant.instructions[0].title,
        Some("kept".to_owned())
    );
}

#[test]
fn a_type_change_under_the_flattened_tool_map_drops_the_whole_subtree() {
    // `serde`'s `flatten` buffers the map it absorbs, and path tracking does
    // not survive that buffer: every failure under `conversation.tools`
    // reports the container. The subtree goes, and the rest of the config
    // stays — which the strip pass above already handles for the far more
    // common unknown-field case.
    let value = json!({
        "assistant": { "model": { "id": "anthropic/claude-sonnet-4" } },
        "style": { "code": { "color": true } },
        "conversation": {
            "tools": {
                "bash": { "source": "local", "summary": { "was": "a string" } }
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert_eq!(
        config.assistant.model.id.to_string(),
        "anthropic/claude-sonnet-4"
    );
    assert_eq!(config.style.code.color, Some(true));
    assert!(config.conversation.tools.tools.is_empty());
}

#[test]
fn partial_config_falls_back_on_non_object() {
    let config = deserialize_partial_config(json!("not an object at all"));
    assert_eq!(config, PartialAppConfig::empty());
}

#[test]
fn partial_config_empty_object() {
    let config = deserialize_partial_config(json!({}));
    assert_eq!(config, PartialAppConfig::empty());
}

#[test]
fn legacy_enable_strings_survive_compat_deserialization() {
    use jp_config::conversation::tool::{AllowToggle, PartialEnableConfig};

    // A stored config (base snapshot or `config_delta`, both routed through
    // this function) with pre-RFD-081 legacy `enable` strings on per-tool
    // entries must still load and map to the canonical shapes.
    let value = json!({
        "conversation": {
            "tools": {
                "describe_tools": { "enable": "always" },
                "dangerous": { "enable": "explicit" },
                "off_tool": { "enable": "off" }
            }
        }
    });

    let config = deserialize_partial_config(value);
    let tools = &config.conversation.tools.tools;

    assert_eq!(
        tools["describe_tools"].enable,
        Some(PartialEnableConfig::LOCKED_ON),
        "legacy `always` must map to locked-on"
    );
    assert_eq!(
        tools["dangerous"].enable,
        Some(PartialEnableConfig {
            state: Some(false),
            allow_toggle: Some(AllowToggle::IfNamed),
        }),
        "legacy `explicit` must map to off-unless-named"
    );
    assert_eq!(tools["off_tool"].enable, Some(PartialEnableConfig::OFF));
}

#[test]
fn legacy_rule_bounds_survive_compat_deserialization() {
    use jp_config::conversation::compaction::RuleBound;

    // A conversation stored before `last` was renamed to `last-compaction`, and
    // before `@N` stopped being a config spelling, must still load. The
    // settings around the stale bound are what a hard failure would cost.
    let value = json!({
        "style": { "code": { "color": false } },
        "conversation": {
            "compaction": {
                "rules": {
                    "value": [
                        { "keep_first": "last", "keep_last": 3 },
                        { "keep_first": "@5", "keep_last": "-4" }
                    ],
                    "strategy": "replace"
                }
            }
        }
    });

    let config = deserialize_partial_config(value);

    assert_eq!(
        config.style.code.color,
        Some(false),
        "an unrelated setting must survive a stale compaction bound"
    );

    // `last` is renamed in place; the `@5` rule goes entirely, because running
    // it with a substituted bound would compact a range it never named.
    let rules = &config.conversation.compaction.rules;
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].keep_first, Some(RuleBound::AfterLastCompaction));
    assert_eq!(rules[0].keep_last, Some(RuleBound::Turns(3)));
}

#[test]
fn legacy_rule_bounds_migrate_in_bare_array_form() {
    use jp_config::conversation::compaction::RuleBound;

    // `rules` is a `MergeableVec`, so a hand-written config or `--cfg` delta
    // can store the bare-array form instead of the `{ value: [...] }` shape
    // `to_parts` writes.
    let value = json!({
        "conversation": {
            "compaction": {
                "rules": [
                    { "keep_first": "LAST", "keep_last": 2 },
                    { "keep_first": "@9" }
                ]
            }
        }
    });

    let rules = deserialize_partial_config(value)
        .conversation
        .compaction
        .rules;
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].keep_first, Some(RuleBound::AfterLastCompaction));
    assert_eq!(rules[0].keep_last, Some(RuleBound::Turns(2)));
}

#[test]
fn dropping_the_only_rule_leaves_an_explicit_empty_rule_set() {
    // An empty bare array reads as "unset" to
    // `PartialCompactionConfig::fill_from`, which answers with the built-in
    // strip-everything rule — a wider range than the rule just dropped. The
    // `Merged` form with a strategy reads as "no rules" instead.
    let mut value = json!({
        "conversation": {
            "compaction": {
                "rules": [{ "keep_first": "@9", "reasoning": "strip" }]
            }
        }
    });

    migrate_legacy_rule_bounds(&mut value);

    assert_eq!(
        value["conversation"]["compaction"]["rules"],
        json!({ "value": [], "strategy": "replace" })
    );
}

#[test]
fn dropping_the_only_rule_in_a_metadata_free_wrapper_leaves_a_strategy() {
    // The object wrapper without a strategy is as "unset" as a bare array
    // once its last rule goes: `MergeableVec::is_empty` looks at the metadata,
    // not at which of the two shapes was written.
    let mut value = json!({
        "conversation": {
            "compaction": {
                "rules": { "value": [{ "keep_first": "@9" }] }
            }
        }
    });

    migrate_legacy_rule_bounds(&mut value);

    assert_eq!(
        value["conversation"]["compaction"]["rules"],
        json!({ "value": [], "strategy": "replace" })
    );
}

#[test]
fn emptying_a_rule_set_keeps_the_strategy_it_already_had() {
    // An emptied `append` list is a no-op against lower layers. Forcing
    // `replace` here would turn it into one that wipes their rules.
    let mut value = json!({
        "conversation": {
            "compaction": {
                "rules": {
                    "value": [{ "keep_first": "@9" }],
                    "strategy": "append"
                }
            }
        }
    });

    migrate_legacy_rule_bounds(&mut value);

    assert_eq!(
        value["conversation"]["compaction"]["rules"],
        json!({ "value": [], "strategy": "append" })
    );
}
