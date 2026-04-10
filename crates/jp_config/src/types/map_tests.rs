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
