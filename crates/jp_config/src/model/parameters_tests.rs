use serde_json::json;

use super::*;
use crate::types::json_value::JsonValue;

#[test]
fn assign_unknown_key_delegates_to_other() {
    let mut p = PartialParametersConfig::default();
    let kv = KvAssignment::try_from_cli("seed", "42").unwrap();
    p.assign(kv).unwrap();

    let other = p.other.as_ref().unwrap();
    assert_eq!(other["seed"], JsonValue(json!("42")));
}

#[test]
fn assign_unknown_nested_key_delegates_to_other() {
    let mut p = PartialParametersConfig::default();
    let kv = KvAssignment::try_from_cli("custom.depth", "3").unwrap();
    p.assign(kv).unwrap();

    let other = p.other.as_ref().unwrap();
    assert_eq!(other["custom"], JsonValue(json!({"depth": "3"})));
}

#[test]
fn stop_words_append_across_layers() {
    use schematic::PartialConfig as _;

    let mut base = PartialParametersConfig {
        stop_words: Some(vec!["STOP".to_owned()]),
        ..Default::default()
    };
    let overlay = PartialParametersConfig {
        stop_words: Some(vec!["HALT".to_owned()]),
        ..Default::default()
    };

    base.merge(&(), overlay).unwrap();

    assert_eq!(
        base.stop_words,
        Some(vec!["STOP".to_owned(), "HALT".to_owned()])
    );
}

#[test]
fn assign_known_keys_not_routed_to_other() {
    let mut p = PartialParametersConfig::default();

    let kv = KvAssignment::try_from_cli("temperature", "0.7").unwrap();
    p.assign(kv).unwrap();
    assert!(p.other.is_none());

    let kv = KvAssignment::try_from_cli("max_tokens", "1024").unwrap();
    p.assign(kv).unwrap();
    assert!(p.other.is_none());
}
