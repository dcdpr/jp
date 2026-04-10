use schematic::PartialConfig as _;
use serde_json::json;

use super::*;

// --- Serde ---

#[test]
fn serde_roundtrip_string() {
    let v = JsonValue(json!("hello"));
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, r#""hello""#);
    let parsed: JsonValue = serde_json::from_str(&json).unwrap();
    assert_eq!(v, parsed);
}

#[test]
fn serde_roundtrip_object() {
    let v = JsonValue(json!({"bind": "0.0.0.0", "port": 8080}));
    let json = serde_json::to_string(&v).unwrap();
    let parsed: JsonValue = serde_json::from_str(&json).unwrap();
    assert_eq!(v, parsed);
}

// --- Default merge (no annotation) ---

#[test]
fn merge_primitives_replaces() {
    let mut base = JsonValue(json!(42));
    base.merge(&(), JsonValue(json!(99))).unwrap();
    assert_eq!(base.0, json!(99));
}

#[test]
fn merge_objects_deep_by_default() {
    let mut base = JsonValue(json!({"web": {"host": "localhost", "port": 3000}}));
    base.merge(&(), JsonValue(json!({"web": {"port": 8080}, "log": true})))
        .unwrap();

    assert_eq!(
        base.0,
        json!({"web": {"host": "localhost", "port": 8080}, "log": true})
    );
}

#[test]
fn merge_nested_objects_three_levels() {
    let mut base = JsonValue(json!({"a": {"b": {"c": 1, "d": 2}}}));
    base.merge(&(), JsonValue(json!({"a": {"b": {"c": 10, "e": 3}}})))
        .unwrap();
    assert_eq!(base.0, json!({"a": {"b": {"c": 10, "d": 2, "e": 3}}}));
}

#[test]
fn merge_object_replaces_primitive() {
    let mut base = JsonValue(json!("old"));
    base.merge(&(), JsonValue(json!({"key": "val"}))).unwrap();
    assert_eq!(base.0, json!({"key": "val"}));
}

#[test]
fn merge_primitive_replaces_object() {
    let mut base = JsonValue(json!({"key": "val"}));
    base.merge(&(), JsonValue(json!(42))).unwrap();
    assert_eq!(base.0, json!(42));
}

#[test]
fn merge_arrays_replace_by_default() {
    let mut base = JsonValue(json!([1, 2]));
    base.merge(&(), JsonValue(json!([3, 4]))).unwrap();
    assert_eq!(base.0, json!([3, 4]));
}

// --- Array strategies ---

#[test]
fn merge_array_append() {
    let mut base = JsonValue(json!([1, 2]));
    base.merge(
        &(),
        JsonValue(json!({"value": [3, 4], "strategy": "append"})),
    )
    .unwrap();
    assert_eq!(base.0, json!([1, 2, 3, 4]));
}

#[test]
fn merge_array_prepend() {
    let mut base = JsonValue(json!([3, 4]));
    base.merge(
        &(),
        JsonValue(json!({"value": [1, 2], "strategy": "prepend"})),
    )
    .unwrap();
    assert_eq!(base.0, json!([1, 2, 3, 4]));
}

#[test]
fn merge_array_replace_explicit() {
    let mut base = JsonValue(json!([1, 2]));
    base.merge(&(), JsonValue(json!({"value": [3], "strategy": "replace"})))
        .unwrap();
    assert_eq!(base.0, json!([3]));
}

#[test]
fn merge_append_on_non_array_falls_back_to_replace() {
    let mut base = JsonValue(json!("not an array"));
    base.merge(
        &(),
        JsonValue(json!({"value": [1, 2], "strategy": "append"})),
    )
    .unwrap();
    assert_eq!(base.0, json!([1, 2]));
}

// --- Object strategies ---

#[test]
fn merge_object_replace_explicit() {
    let mut base = JsonValue(json!({"a": 1, "b": 2}));
    base.merge(
        &(),
        JsonValue(json!({"value": {"a": 10}, "strategy": "replace"})),
    )
    .unwrap();
    assert_eq!(base.0, json!({"a": 10}));
}

#[test]
fn merge_object_deep_merge_explicit() {
    let mut base = JsonValue(json!({"a": {"x": 1}, "b": 2}));
    base.merge(
        &(),
        JsonValue(json!({"value": {"a": {"y": 3}, "c": 4}, "strategy": "deep_merge"})),
    )
    .unwrap();
    assert_eq!(base.0, json!({"a": {"x": 1, "y": 3}, "b": 2, "c": 4}));
}

#[test]
fn merge_object_shallow_merge() {
    let mut base = JsonValue(json!({"a": {"x": 1}, "b": 2}));
    base.merge(
        &(),
        JsonValue(json!({"value": {"a": {"y": 3}, "c": 4}, "strategy": "merge"})),
    )
    .unwrap();
    assert_eq!(base.0, json!({"a": {"y": 3}, "b": 2, "c": 4}));
}

#[test]
fn merge_object_keep() {
    let mut base = JsonValue(json!({"a": {"x": 1}, "b": 2}));
    base.merge(
        &(),
        JsonValue(json!({"value": {"a": {"y": 3}, "c": 4}, "strategy": "keep"})),
    )
    .unwrap();
    assert_eq!(base.0, json!({"a": {"x": 1}, "b": 2, "c": 4}));
}

#[test]
fn merge_object_keep_fills_null_base() {
    let mut base = JsonValue(json!(null));
    base.merge(
        &(),
        JsonValue(json!({"value": {"a": 1}, "strategy": "keep"})),
    )
    .unwrap();
    assert_eq!(base.0, json!({"a": 1}));
}

// --- String strategies ---

#[test]
fn merge_string_replace() {
    let mut base = JsonValue(json!("hello"));
    base.merge(
        &(),
        JsonValue(json!({"value": "world", "strategy": "replace"})),
    )
    .unwrap();
    assert_eq!(base.0, json!("world"));
}

#[test]
fn merge_string_append() {
    let mut base = JsonValue(json!("hello"));
    base.merge(
        &(),
        JsonValue(json!({"value": " world", "strategy": "append"})),
    )
    .unwrap();
    assert_eq!(base.0, json!("hello world"));
}

#[test]
fn merge_string_prepend() {
    let mut base = JsonValue(json!("world"));
    base.merge(
        &(),
        JsonValue(json!({"value": "hello ", "strategy": "prepend"})),
    )
    .unwrap();
    assert_eq!(base.0, json!("hello world"));
}

#[test]
fn merge_string_append_with_separator() {
    let mut base = JsonValue(json!("line1"));
    base.merge(
        &(),
        JsonValue(json!({"value": "line2", "strategy": "append", "separator": "line"})),
    )
    .unwrap();
    assert_eq!(base.0, json!("line1\nline2"));
}

#[test]
fn merge_string_prepend_with_separator() {
    let mut base = JsonValue(json!("line2"));
    base.merge(
        &(),
        JsonValue(json!({"value": "line1", "strategy": "prepend", "separator": "paragraph"})),
    )
    .unwrap();
    assert_eq!(base.0, json!("line1\n\nline2"));
}

#[test]
fn merge_string_append_to_non_string_replaces() {
    let mut base = JsonValue(json!(42));
    base.merge(
        &(),
        JsonValue(json!({"value": "hello", "strategy": "append"})),
    )
    .unwrap();
    assert_eq!(base.0, json!("hello"));
}

// --- Nested annotation inside object ---

#[test]
fn merge_annotation_inside_nested_object() {
    let mut base = JsonValue(json!({"tags": ["a", "b"], "name": "test"}));
    base.merge(
        &(),
        JsonValue(json!({
            "tags": {"value": ["c"], "strategy": "append"},
            "name": "updated"
        })),
    )
    .unwrap();
    assert_eq!(base.0, json!({"tags": ["a", "b", "c"], "name": "updated"}));
}

// --- Non-annotation detection ---

#[test]
fn non_annotation_object_with_extra_keys_left_alone() {
    let mut base = JsonValue(json!({}));
    let next = JsonValue(json!({"value": 1, "strategy": "append", "extra": true}));
    base.merge(&(), next).unwrap();
    assert_eq!(
        base.0,
        json!({"value": 1, "strategy": "append", "extra": true})
    );
}

#[test]
fn object_with_unknown_strategy_left_alone() {
    let mut base = JsonValue(json!({}));
    let next = JsonValue(json!({"value": 1, "strategy": "unknown"}));
    base.merge(&(), next).unwrap();
    assert_eq!(base.0, json!({"value": 1, "strategy": "unknown"}));
}

// --- Finalize (annotation stripping) ---

#[test]
fn finalize_strips_array_annotation() {
    let v = JsonValue(json!({"value": [1, 2], "strategy": "append"}));
    let v = v.finalize(&()).unwrap();
    assert_eq!(v.0, json!([1, 2]));
}

#[test]
fn finalize_strips_object_annotation() {
    let v = JsonValue(json!({
        "server": {"value": {"port": 3000}, "strategy": "keep"},
    }));
    let v = v.finalize(&()).unwrap();
    assert_eq!(v.0, json!({"server": {"port": 3000}}));
}

#[test]
fn finalize_strips_string_annotation_with_separator() {
    let v = JsonValue(json!({"value": "extra", "strategy": "append", "separator": "line"}));
    let v = v.finalize(&()).unwrap();
    assert_eq!(v.0, json!("extra"));
}

#[test]
fn finalize_preserves_non_annotation_objects() {
    let v = JsonValue(json!({"value": 1, "strategy": "append", "extra": true}));
    let v = v.finalize(&()).unwrap();
    assert_eq!(
        v.0,
        json!({"value": 1, "strategy": "append", "extra": true})
    );
}

// --- AssignKeyValue ---

#[test]
fn assign_terminal_sets_value() {
    let mut v = JsonValue::default();
    let kv = KvAssignment::try_from_cli("", "hello").unwrap();
    v.assign(kv).unwrap();
    assert_eq!(v.0, json!("hello"));
}

#[test]
fn assign_one_level_creates_object() {
    let mut v = JsonValue::default();
    let kv = KvAssignment::try_from_cli("port", "3000").unwrap();
    v.assign(kv).unwrap();
    assert_eq!(v.0, json!({"port": "3000"}));
}

#[test]
fn assign_nested_creates_deep_object() {
    let mut v = JsonValue::default();
    let kv = KvAssignment::try_from_cli("web.port", "3000").unwrap();
    v.assign(kv).unwrap();
    assert_eq!(v.0, json!({"web": {"port": "3000"}}));
}

#[test]
fn assign_preserves_existing_siblings() {
    let mut v = JsonValue(json!({"web": {"host": "localhost"}}));
    let kv = KvAssignment::try_from_cli("web.port", "3000").unwrap();
    v.assign(kv).unwrap();
    assert_eq!(v.0, json!({"web": {"host": "localhost", "port": "3000"}}));
}

#[test]
fn assign_json_object_merges() {
    let mut v = JsonValue(json!({"host": "localhost"}));
    let kv = KvAssignment::try_from_cli(":", r#"{"port": 3000}"#).unwrap();
    v.assign(kv).unwrap();
    assert_eq!(v.0, json!({"host": "localhost", "port": 3000}));
}

#[test]
fn assign_overwrites_primitive_with_nested() {
    let mut v = JsonValue(json!("old"));
    let kv = KvAssignment::try_from_cli("nested.key", "val").unwrap();
    v.assign(kv).unwrap();
    assert_eq!(v.0, json!({"nested": {"key": "val"}}));
}

// --- Deref / Default ---

#[test]
fn deref_to_value() {
    let v = JsonValue(json!({"port": 3000}));
    assert_eq!(v.get("port").and_then(Value::as_u64), Some(3000));
}

#[test]
fn default_is_null() {
    assert_eq!(JsonValue::default().0, Value::Null);
}

#[test]
fn empty_is_null() {
    assert!(JsonValue::empty().is_empty());
    assert!(!JsonValue(json!(0)).is_empty());
    assert!(!JsonValue(json!(false)).is_empty());
}
