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
