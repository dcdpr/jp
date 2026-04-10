use serde_json::json;

use super::*;
use crate::assignment::KvAssignment;

#[test]
fn assign_flat_value() {
    let mut p = PartialTemplateConfig::default();

    let kv = KvAssignment::try_from_cli("values.name", "Homer").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(p.values["name"], JsonValue(json!("Homer")));
}

#[test]
fn assign_nested_value() {
    let mut p = PartialTemplateConfig::default();

    let kv = KvAssignment::try_from_cli("values.user.name", "Homer").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(p.values["user"], JsonValue(json!({"name": "Homer"})));
}

#[test]
fn assign_preserves_existing() {
    let mut p = PartialTemplateConfig::default();

    let kv = KvAssignment::try_from_cli("values.a", "1").unwrap();
    p.assign(kv).unwrap();
    let kv = KvAssignment::try_from_cli("values.b", "2").unwrap();
    p.assign(kv).unwrap();

    assert_eq!(p.values["a"], JsonValue(json!("1")));
    assert_eq!(p.values["b"], JsonValue(json!("2")));
}
