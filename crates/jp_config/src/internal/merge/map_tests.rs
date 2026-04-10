use indexmap::IndexMap;
use serde_json::json;

use super::*;
use crate::types::{
    json_value::JsonValue,
    map::{MergeableMap, MergedMap, MergedMapStrategy},
};

fn jv(v: serde_json::Value) -> JsonValue {
    JsonValue(v)
}

fn make_map(pairs: &[(&str, serde_json::Value)]) -> MergeableMap<JsonValue> {
    MergeableMap::Map(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), jv(v.clone())))
            .collect(),
    )
}

#[test]
fn deep_merge_default() {
    let prev = make_map(&[("a", json!({"x": 1})), ("b", json!(2))]);
    let next = make_map(&[("a", json!({"y": 3})), ("c", json!(4))]);

    let result = map_with_strategy(prev, next, &()).unwrap().unwrap();
    let map = result.into_map();

    assert_eq!(map["a"], jv(json!({"x": 1, "y": 3})));
    assert_eq!(map["b"], jv(json!(2)));
    assert_eq!(map["c"], jv(json!(4)));
}

#[test]
fn shallow_merge() {
    let prev = make_map(&[("a", json!({"x": 1})), ("b", json!(2))]);
    let next = MergeableMap::Merged(MergedMap {
        value: IndexMap::from([
            ("a".to_owned(), jv(json!({"y": 3}))),
            ("c".to_owned(), jv(json!(4))),
        ]),
        strategy: Some(MergedMapStrategy::Merge),
        discard_when_merged: false,
    });

    let result = map_with_strategy(prev, next, &()).unwrap().unwrap();
    let map = result.into_map();

    assert_eq!(map["a"], jv(json!({"y": 3})));
    assert_eq!(map["b"], jv(json!(2)));
    assert_eq!(map["c"], jv(json!(4)));
}

#[test]
fn keep_merge() {
    let prev = make_map(&[("a", json!(1)), ("b", json!(2))]);
    let next = MergeableMap::Merged(MergedMap {
        value: IndexMap::from([
            ("a".to_owned(), jv(json!(10))),
            ("c".to_owned(), jv(json!(3))),
        ]),
        strategy: Some(MergedMapStrategy::Keep),
        discard_when_merged: false,
    });

    let result = map_with_strategy(prev, next, &()).unwrap().unwrap();
    let map = result.into_map();

    assert_eq!(map["a"], jv(json!(1)));
    assert_eq!(map["b"], jv(json!(2)));
    assert_eq!(map["c"], jv(json!(3)));
}

#[test]
fn replace_strategy() {
    let prev = make_map(&[("a", json!(1)), ("b", json!(2))]);
    let next = MergeableMap::Merged(MergedMap {
        value: IndexMap::from([("c".to_owned(), jv(json!(3)))]),
        strategy: Some(MergedMapStrategy::Replace),
        discard_when_merged: false,
    });

    let result = map_with_strategy(prev, next, &()).unwrap().unwrap();
    let map = result.into_map();

    assert_eq!(map.len(), 1);
    assert_eq!(map["c"], jv(json!(3)));
}

#[test]
fn discard_when_merged_replaces_regardless() {
    let prev = MergeableMap::Merged(MergedMap {
        value: IndexMap::from([("default".to_owned(), jv(json!(true)))]),
        strategy: Some(MergedMapStrategy::Keep),
        discard_when_merged: true,
    });
    let next = make_map(&[("real", json!(42))]);

    let result = map_with_strategy(prev, next, &()).unwrap().unwrap();
    let map = result.into_map();

    assert_eq!(map.len(), 1);
    assert_eq!(map["real"], jv(json!(42)));
}
