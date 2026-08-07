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
fn partial_config_falls_back_on_type_mismatch() {
    // `color` expects a bool, but we give it an array. After stripping
    // (which won't help since `color` is a known field with a wrong type),
    // deserialization should fail and we get an empty config.
    let value = json!({
        "style": {
            "code": {
                "color": [1, 2, 3]
            }
        }
    });

    let config = deserialize_partial_config(value);
    assert!(config.style.code.color.is_none());
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
