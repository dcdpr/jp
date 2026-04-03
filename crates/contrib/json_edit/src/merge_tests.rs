use indoc::indoc;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::ast::Document;

#[test]
fn merge_adds_new_key() {
    let doc = Document::parse(r#"{"a": 1}"#).unwrap();
    deep_merge(&doc, &json!({"b": 2})).unwrap();
    // Key order: "a" preserved, "b" appended
    let result = doc.to_string();
    assert!(result.contains("\"a\""));
    assert!(result.contains("\"b\""));
}

#[test]
fn merge_overwrites_existing_key() {
    let doc = Document::parse(r#"{"key": "old"}"#).unwrap();
    deep_merge(&doc, &json!({"key": "new"})).unwrap();
    assert_eq!(doc.to_string(), r#"{"key": "new"}"#);
}

#[test]
fn merge_recurses_into_nested_objects() {
    let input = r#"{"parent": {"keep": 1, "change": "old"}}"#;
    let doc = Document::parse(input).unwrap();
    deep_merge(&doc, &json!({"parent": {"change": "new"}})).unwrap();

    let result = doc.to_string();
    assert!(result.contains("\"keep\""), "untouched key preserved");
    assert!(result.contains("\"new\""), "changed value updated");
    assert!(!result.contains("\"old\""), "old value gone");
}

#[test]
fn merge_replaces_non_object_with_value() {
    let doc = Document::parse(r#"{"key": 42}"#).unwrap();
    deep_merge(&doc, &json!({"key": "hello"})).unwrap();
    assert_eq!(doc.to_string(), r#"{"key": "hello"}"#);
}

#[test]
fn merge_preserves_formatting() {
    let input = indoc! {r#"
        {
          "first": 1,
          "second": 2
        }
    "#}
    .trim();
    let doc = Document::parse(input).unwrap();
    deep_merge(&doc, &json!({"second": 99})).unwrap();

    let expected = indoc! {r#"
        {
          "first": 1,
          "second": 99
        }
    "#}
    .trim();
    assert_eq!(doc.to_string(), expected);
}

#[test]
fn merge_preserves_json5_comments() {
    let input = indoc! {r#"
        {
          // Important setting
          "key": "value"
        }
    "#}
    .trim();
    let doc = Document::parse_json5(input).unwrap();
    deep_merge(&doc, &json!({"key": "updated"})).unwrap();
    let result = doc.to_string();
    assert!(result.contains("// Important setting"), "comment preserved");
    assert!(result.contains("\"updated\""), "value updated");
}

#[test]
fn merge_empty_source_is_noop() {
    let input = r#"{"a": 1}"#;
    let doc = Document::parse(input).unwrap();
    deep_merge(&doc, &json!({})).unwrap();
    assert_eq!(doc.to_string(), input);
}

#[test]
fn merge_into_empty_object() {
    let doc = Document::parse("{}").unwrap();
    deep_merge(&doc, &json!({"key": "value"})).unwrap();
    let result = doc.to_string();
    assert!(result.contains("\"key\""));
    assert!(result.contains("\"value\""));
}

#[test]
fn merge_rejects_non_object_root() {
    let doc = Document::parse("[1, 2]").unwrap();
    let err = deep_merge(&doc, &json!({"key": 1}));
    assert_eq!(err, Err(MergeError::RootNotObject));
}

#[test]
fn merge_rejects_non_object_source() {
    let doc = Document::parse("{}").unwrap();
    let err = deep_merge(&doc, &json!(42));
    assert_eq!(err, Err(MergeError::SourceNotObject));
}

#[test]
fn merge_deeply_nested() {
    let input = r#"{"a": {"b": {"c": "old"}}}"#;
    let doc = Document::parse(input).unwrap();
    deep_merge(&doc, &json!({"a": {"b": {"c": "new"}}})).unwrap();
    let result = doc.to_string();
    assert!(result.contains("\"new\""));
    assert!(!result.contains("\"old\""));
}
