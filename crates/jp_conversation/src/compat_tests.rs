use jp_config::{
    AppConfig, PartialAppConfig, Schema,
    conversation::tool::{PartialToolConfig, RunMode},
    schema::{BooleanType, StructType},
};
use serde_json::json;

use super::*;
use crate::{
    ConversationStream,
    event::ChatRequest,
    stream::{ConfigDelta, InternalEvent},
};

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
fn deserialize_config_delta_with_injected_unknown() {
    // Call deserialize_config_delta directly with a hand-built JSON value
    // that has an unknown field in style.code.
    let value = json!({
        "timestamp": "2025-01-01 00:00:00.0",
        "delta": {
            "style": {
                "code": {
                    "color": false,
                    "removed_field": "stale"
                }
            }
        }
    });

    let delta = deserialize_config_delta(value);
    assert_eq!(
        delta.delta.style.code.color,
        Some(false),
        "known field 'color' should survive"
    );
}

/// Serialize a [`ConfigDelta`] as an [`InternalEvent`] and deserialize it back,
/// exercising the real code path.
fn roundtrip_delta(delta: ConfigDelta) -> ConfigDelta {
    let event = InternalEvent::ConfigDelta(delta);
    let json = serde_json::to_value(&event).unwrap();
    let deserialized: InternalEvent = serde_json::from_value(json).unwrap();
    match deserialized {
        InternalEvent::ConfigDelta(d) => d,
        InternalEvent::Event(_) => panic!("expected ConfigDelta"),
    }
}

#[test]
fn roundtrip_default_config_preserves_all_fields() {
    let original = ConfigDelta::from(AppConfig::new_test().to_partial());
    let result = roundtrip_delta(original.clone());

    // Compare via JSON because some in-memory types (e.g. MergedVec)
    // don't survive roundtrip as the same Rust variant but serialize
    // identically.
    let original_json = serde_json::to_value(&original).unwrap();
    let result_json = serde_json::to_value(&result).unwrap();
    assert_eq!(original_json, result_json);
}

#[test]
fn roundtrip_empty_delta() {
    let original = ConfigDelta::from(PartialAppConfig::empty());
    let result = roundtrip_delta(original.clone());
    assert_eq!(original, result);
}

#[test]
fn roundtrip_delta_with_tool_defaults() {
    let mut partial = PartialAppConfig::empty();
    partial.conversation.tools.defaults.run = Some(RunMode::Unattended);

    let original = ConfigDelta::from(partial);
    let result = roundtrip_delta(original.clone());

    let original_json = serde_json::to_value(&original).unwrap();
    let result_json = serde_json::to_value(&result).unwrap();
    assert_eq!(original_json, result_json);
}

/// This is the critical test: per-tool entries are serialized as flattened keys
/// alongside "*" in the tools object. The schema only knows about "*", so a
/// naive strip would remove all per-tool config.
#[test]
fn roundtrip_delta_with_per_tool_overrides() {
    let mut partial = PartialAppConfig::empty();
    partial.conversation.tools.defaults.run = Some(RunMode::Ask);

    let tool = PartialToolConfig {
        run: Some(RunMode::Unattended),
        ..Default::default()
    };
    partial
        .conversation
        .tools
        .tools
        .insert("fs_read_file".into(), tool);

    let tool2 = PartialToolConfig {
        run: Some(RunMode::Unattended),
        ..Default::default()
    };
    partial
        .conversation
        .tools
        .tools
        .insert("cargo_check".into(), tool2);

    let original = ConfigDelta::from(partial);
    let result = roundtrip_delta(original.clone());

    let original_json = serde_json::to_value(&original).unwrap();
    let result_json = serde_json::to_value(&result).unwrap();
    assert_eq!(original_json, result_json);
}

#[test]
fn roundtrip_delta_strip_unknown_field_preserves_rest() {
    // Serialize a real delta, inject an unknown field, then deserialize.
    let mut partial = PartialAppConfig::empty();
    partial.style.code.color = Some(false);
    let original = ConfigDelta::from(partial);

    let event = InternalEvent::ConfigDelta(original);
    let mut json = serde_json::to_value(&event).unwrap();

    // Inject unknown field next to `color`
    json["delta"]["style"]["code"]["removed_field"] = json!("stale");

    let deserialized: InternalEvent = serde_json::from_value(json).unwrap();
    let InternalEvent::ConfigDelta(result) = deserialized else {
        panic!("expected ConfigDelta");
    };

    // The known field survived
    assert_eq!(result.delta.style.code.color, Some(false));
}

/// End-to-end: build a `ConversationStream` with tool config, serialize it,
/// inject unknown fields into the config deltas, then deserialize.
#[test]
fn roundtrip_full_stream_with_tools_and_unknown_fields() {
    let mut partial = PartialAppConfig::empty();
    partial.conversation.tools.defaults.run = Some(RunMode::Unattended);
    partial.style.code.color = Some(false);

    let mut stream = ConversationStream::new_test().with_config_delta(partial);
    stream.start_turn(ChatRequest::from("hello"));

    let mut json = serde_json::to_value(&stream).unwrap();

    // Inject unknown fields into both config deltas (base + the one we added)
    let events = json.as_array_mut().unwrap();
    for event in events.iter_mut() {
        if event.get("type").and_then(|v| v.as_str()) == Some("config_delta")
            && let Some(delta) = event.get_mut("delta")
        {
            if let Some(obj) = delta.as_object_mut() {
                obj.insert("removed_top_field".into(), json!("stale"));
            }
            if let Some(code) = delta.pointer_mut("/style/code")
                && let Some(obj) = code.as_object_mut()
            {
                obj.insert("removed_code_field".into(), json!("old_theme"));
            }
        }
    }

    // Deserialize should succeed despite unknown fields
    let result: ConversationStream = serde_json::from_value(json).unwrap();

    // The stream should have the same structure
    assert_eq!(result.len(), stream.len());

    // The tool config should have survived
    let config = result.config().unwrap();
    assert_eq!(config.conversation.tools.defaults.run, RunMode::Unattended);
    assert!(!config.style.code.color);
}

/// Multiple deltas each set a field that no longer exists. All should be
/// stripped, and the stream should load fine.
#[test]
fn roundtrip_stream_with_multiple_deltas_all_referencing_removed_field() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("hello"));

    let mut json = serde_json::to_value(&stream).unwrap();

    // Inject two extra config_delta events with only unknown fields
    let extra_delta_1 = json!({
        "type": "config_delta",
        "timestamp": "2025-01-01 00:01:00.0",
        "delta": {
            "removed_section": { "a": 1 }
        }
    });
    let extra_delta_2 = json!({
        "type": "config_delta",
        "timestamp": "2025-01-01 00:02:00.0",
        "delta": {
            "removed_section": { "a": 2 }
        }
    });

    let events = json.as_array_mut().unwrap();
    events.push(extra_delta_1);
    events.push(extra_delta_2);

    let result: ConversationStream = serde_json::from_value(json).unwrap();
    // Stream still loads and has our original event
    assert_eq!(result.len(), 2); // TurnStart + ChatRequest
}

#[test]
fn deserialize_valid_delta() {
    let value = json!({
        "timestamp": "2025-01-01 00:00:00.0",
        "delta": {
            "style": {
                "code": {
                    "color": false
                }
            }
        }
    });

    let delta = deserialize_config_delta(value);
    assert_eq!(delta.timestamp.to_string(), "2025-01-01 00:00:00 UTC");
    assert_eq!(delta.delta.style.code.color, Some(false));
}

#[test]
fn deserialize_strips_unknown_fields_and_preserves_known() {
    let value = json!({
        "timestamp": "2025-06-15 12:30:00.0",
        "delta": {
            "style": {
                "code": {
                    "color": true,
                    "theme": "dracula"
                }
            }
        }
    });

    let delta = deserialize_config_delta(value);
    assert_eq!(delta.delta.style.code.color, Some(true));
}

#[test]
fn deserialize_falls_back_on_type_mismatch() {
    // `color` expects a bool, but we give it an array. After stripping
    // (which won't help since `color` is a known field with a wrong type),
    // deserialization should fail and we get an empty delta.
    let value = json!({
        "timestamp": "2025-03-20 08:00:00.0",
        "delta": {
            "style": {
                "code": {
                    "color": [1, 2, 3]
                }
            }
        }
    });

    let delta = deserialize_config_delta(value);
    assert_eq!(delta.timestamp.to_string(), "2025-03-20 08:00:00 UTC");
    assert!(delta.delta.style.code.color.is_none());
}

#[test]
fn deserialize_falls_back_preserving_timestamp() {
    let value = json!({
        "timestamp": "2024-12-25 18:30:00.0",
        "delta": "not an object at all"
    });

    let delta = deserialize_config_delta(value);
    assert_eq!(delta.timestamp.to_string(), "2024-12-25 18:30:00 UTC");
}

#[test]
fn deserialize_empty_delta() {
    let value = json!({
        "timestamp": "2025-01-01 00:00:00.0",
        "delta": {}
    });

    let delta = deserialize_config_delta(value);
    assert_eq!(delta.timestamp.to_string(), "2025-01-01 00:00:00 UTC");
}
