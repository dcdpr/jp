use serde_json::json;

use super::*;

#[test]
fn deserialize_plain_map() {
    let v: MergeableMap<serde_json::Value> =
        serde_json::from_value(json!({"a": 1, "b": 2})).unwrap();
    assert!(matches!(v, MergeableMap::Map(_)));
    assert_eq!(v["a"], json!(1));
}

#[test]
fn deserialize_merged_map() {
    let v: MergeableMap<serde_json::Value> =
        serde_json::from_value(json!({"value": {"a": 1}, "strategy": "keep"})).unwrap();
    assert!(matches!(v, MergeableMap::Merged(_)));
    assert_eq!(v["a"], json!(1));
}

#[test]
fn deserialize_ambiguous_falls_through_to_merged() {
    // Has "value" and "strategy" keys, so it's detected as Merged.
    let v: MergeableMap<serde_json::Value> =
        serde_json::from_value(json!({"value": {"x": 1}, "strategy": "replace"})).unwrap();
    assert!(matches!(v, MergeableMap::Merged(_)));
}

#[test]
fn into_map_plain() {
    let v: MergeableMap<i32> = MergeableMap::Map(IndexMap::from([("a".into(), 1)]));
    let map = v.into_map();
    assert_eq!(map["a"], 1);
}

#[test]
fn into_map_merged() {
    let v: MergeableMap<i32> = MergeableMap::Merged(MergedMap {
        value: IndexMap::from([("a".into(), 1)]),
        strategy: Some(MergedMapStrategy::Keep),
        discard_when_merged: false,
    });
    let map = v.into_map();
    assert_eq!(map["a"], 1);
}

#[test]
fn is_empty_plain_empty() {
    let v: MergeableMap<i32> = MergeableMap::Map(IndexMap::new());
    assert!(v.is_empty());
}

#[test]
fn is_empty_plain_non_empty() {
    let v: MergeableMap<i32> = MergeableMap::Map(IndexMap::from([("a".into(), 1)]));
    assert!(!v.is_empty());
}

#[test]
fn is_empty_merged_with_metadata() {
    // Empty map but with strategy set — NOT empty.
    let v: MergeableMap<i32> = MergeableMap::Merged(MergedMap {
        value: IndexMap::new(),
        strategy: Some(MergedMapStrategy::Replace),
        discard_when_merged: false,
    });
    assert!(!v.is_empty());
}

#[test]
fn is_empty_merged_no_metadata() {
    // Empty map with no metadata — IS empty.
    let v: MergeableMap<i32> = MergeableMap::Merged(MergedMap {
        value: IndexMap::new(),
        strategy: None,
        discard_when_merged: false,
    });
    assert!(v.is_empty());
}

#[test]
fn is_empty_merged_discard_when_merged() {
    // Empty map but discard_when_merged set — NOT empty.
    let v: MergeableMap<i32> = MergeableMap::Merged(MergedMap {
        value: IndexMap::new(),
        strategy: None,
        discard_when_merged: true,
    });
    assert!(!v.is_empty());
}
