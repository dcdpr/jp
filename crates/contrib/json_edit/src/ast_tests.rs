use indoc::indoc;
use pretty_assertions::assert_eq;

use super::*;

// ---------------------------------------------------------------------------
// Round-trip fidelity
// ---------------------------------------------------------------------------

#[test]
fn display_reproduces_input_compact() {
    let input = r#"{"a":1,"b":"hello","c":true}"#;
    let doc = Document::parse(input).unwrap();
    assert_eq!(doc.to_string(), input);
}

#[test]
fn display_reproduces_input_pretty() {
    let input = indoc! {r#"
        {
          "first": 1,
          "second": "two",
          "third": false
        }
    "#}
    .trim();
    let doc = Document::parse(input).unwrap();
    assert_eq!(doc.to_string(), input);
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

#[test]
fn get_string_value() {
    let doc = Document::parse(r#"{"key": "value"}"#).unwrap();
    let obj = doc.as_object().unwrap();
    let val = obj.get("key").unwrap();
    assert_eq!(val.text(), "\"value\"");
}

#[test]
fn get_number_value() {
    let doc = Document::parse(r#"{"n": 42}"#).unwrap();
    let obj = doc.as_object().unwrap();
    let val = obj.get("n").unwrap();
    assert_eq!(val.text(), "42");
}

#[test]
fn get_missing_key_returns_none() {
    let doc = Document::parse(r#"{"a": 1}"#).unwrap();
    let obj = doc.as_object().unwrap();
    assert!(obj.get("b").is_none());
}

#[test]
fn get_nested_object() {
    let doc = Document::parse(r#"{"outer": {"inner": 1}}"#).unwrap();
    let obj = doc.as_object().unwrap();
    let nested = obj.get_object("outer").unwrap();
    let val = nested.get("inner").unwrap();
    assert_eq!(val.text(), "1");
}

#[test]
fn members_iteration() {
    let doc = Document::parse(r#"{"a": 1, "b": 2, "c": 3}"#).unwrap();
    let obj = doc.as_object().unwrap();
    let keys: Vec<_> = obj.members().filter_map(|m| m.key()).collect();
    assert_eq!(keys, vec!["a", "b", "c"]);
}

// ---------------------------------------------------------------------------
// Object::set - replacement
// ---------------------------------------------------------------------------

#[test]
fn set_replaces_existing_value() {
    let doc = Document::parse(r#"{"key": "old"}"#).unwrap();
    let obj = doc.as_object().unwrap();
    obj.set("key", "\"new\"");
    assert_eq!(doc.to_string(), r#"{"key": "new"}"#);
}

#[test]
fn set_replaces_number_with_string() {
    let doc = Document::parse(r#"{"key": 42}"#).unwrap();
    let obj = doc.as_object().unwrap();
    obj.set("key", "\"hello\"");
    assert_eq!(doc.to_string(), r#"{"key": "hello"}"#);
}

#[test]
fn set_preserves_surrounding_formatting() {
    let input = indoc! {r#"
        {
          "a": 1,
          "b": 2,
          "c": 3
        }
    "#}
    .trim();
    let doc = Document::parse(input).unwrap();
    let obj = doc.as_object().unwrap();
    obj.set("b", "99");

    let expected = indoc! {r#"
        {
          "a": 1,
          "b": 99,
          "c": 3
        }
    "#}
    .trim();
    assert_eq!(doc.to_string(), expected);
}

// ---------------------------------------------------------------------------
// Object::set - insertion
// ---------------------------------------------------------------------------

#[test]
fn set_inserts_new_key_into_empty_object() {
    let doc = Document::parse("{}").unwrap();
    let obj = doc.as_object().unwrap();
    obj.set("key", "\"value\"");
    assert_eq!(doc.to_string(), r#"{"key": "value"}"#);
}

#[test]
fn set_inserts_new_key_compact() {
    let doc = Document::parse(r#"{"a": 1}"#).unwrap();
    let obj = doc.as_object().unwrap();
    obj.set("b", "2");
    assert_eq!(doc.to_string(), r#"{"a": 1,"b": 2}"#);
}

#[test]
fn set_inserts_new_key_multiline() {
    let input = indoc! {r#"
        {
          "a": 1
        }
    "#}
    .trim();
    let doc = Document::parse(input).unwrap();
    let obj = doc.as_object().unwrap();
    obj.set("b", "2");

    let expected = indoc! {r#"
        {
          "a": 1,
          "b": 2
        }
    "#}
    .trim();
    assert_eq!(doc.to_string(), expected);
}

// ---------------------------------------------------------------------------
// Object::remove
// ---------------------------------------------------------------------------

#[test]
fn remove_only_member() {
    let doc = Document::parse(r#"{"a": 1}"#).unwrap();
    let obj = doc.as_object().unwrap();
    assert!(obj.remove("a"));
    assert_eq!(doc.to_string(), "{}");
}

#[test]
fn remove_first_member() {
    let doc = Document::parse(r#"{"a": 1, "b": 2}"#).unwrap();
    let obj = doc.as_object().unwrap();
    assert!(obj.remove("a"));
    assert_eq!(doc.to_string(), r#"{"b": 2}"#);
}

#[test]
fn remove_last_member() {
    let doc = Document::parse(r#"{"a": 1, "b": 2}"#).unwrap();
    let obj = doc.as_object().unwrap();
    assert!(obj.remove("b"));
    assert_eq!(doc.to_string(), r#"{"a": 1}"#);
}

#[test]
fn remove_nonexistent_returns_false() {
    let doc = Document::parse(r#"{"a": 1}"#).unwrap();
    let obj = doc.as_object().unwrap();
    assert!(!obj.remove("z"));
    assert_eq!(doc.to_string(), r#"{"a": 1}"#);
}

// ---------------------------------------------------------------------------
// JSON5 support
// ---------------------------------------------------------------------------

#[test]
fn json5_comment_preservation() {
    let input = "{\n  // This is a comment\n  \"key\": \"value\"\n}";
    let doc = Document::parse_json5(input).unwrap();
    let obj = doc.as_object().unwrap();
    obj.set("key", "\"new\"");
    let result = doc.to_string();
    assert!(result.contains("// This is a comment"), "comment preserved");
    assert!(result.contains("\"new\""), "value updated");
}

#[test]
fn json5_unquoted_key_read() {
    let doc = Document::parse_json5("{key: 42}").unwrap();
    let obj = doc.as_object().unwrap();
    let val = obj.get("key").unwrap();
    assert_eq!(val.text(), "42");
}

// ---------------------------------------------------------------------------
// Array reading
// ---------------------------------------------------------------------------

#[test]
fn array_count() {
    let doc = Document::parse("[1, 2, 3]").unwrap();
    let arr = doc.as_array().unwrap();
    assert_eq!(arr.count(), 3);
}

// ---------------------------------------------------------------------------
// Error handling
// ---------------------------------------------------------------------------

#[test]
fn parse_error_on_invalid_json() {
    let result = Document::parse("{invalid}");
    assert!(result.is_err());
}
