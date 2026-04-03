use pretty_assertions::assert_eq;

use super::*;
use crate::{lexer::Dialect, syntax::SyntaxNode};

fn parse_json(input: &str) -> SyntaxNode {
    let result = parse(input, Dialect::Json);
    SyntaxNode::new_root(result.green_node)
}

fn parse_json5(input: &str) -> SyntaxNode {
    let result = parse(input, Dialect::Json5);
    SyntaxNode::new_root(result.green_node)
}

#[test]
fn lossless_roundtrip_simple_object() {
    let input = r#"{"a": 1, "b": true}"#;
    let root = parse_json(input);
    assert_eq!(root.to_string(), input);
}

#[test]
fn lossless_roundtrip_nested() {
    let input = "{\n  \"a\": {\n    \"b\": [1, 2, 3]\n  }\n}";
    let root = parse_json(input);
    assert_eq!(root.to_string(), input);
}

#[test]
fn lossless_roundtrip_json5_comments() {
    let input = "{\n  // comment\n  key: 'value',\n}";
    let root = parse_json5(input);
    assert_eq!(root.to_string(), input);
}

#[test]
fn lossless_roundtrip_empty_object() {
    let input = "{}";
    let root = parse_json(input);
    assert_eq!(root.to_string(), input);
}

#[test]
fn lossless_roundtrip_empty_array() {
    let input = "[]";
    let root = parse_json(input);
    assert_eq!(root.to_string(), input);
}

#[test]
fn lossless_roundtrip_literals() {
    for input in ["true", "false", "null", "42", "-3.14", r#""hello""#] {
        let root = parse_json(input);
        assert_eq!(root.to_string(), input, "failed for: {input}");
    }
}

#[test]
fn lossless_roundtrip_trailing_comma_json5() {
    let input = "{\"a\": 1, \"b\": 2,}";
    let root = parse_json5(input);
    assert_eq!(root.to_string(), input);
}

#[test]
fn tree_structure_object() {
    let input = r#"{"key": "val"}"#;
    let root = parse_json(input);

    // ROOT -> OBJECT -> MEMBER
    assert_eq!(root.kind(), SyntaxKind::Root);
    let object = root.first_child().unwrap();
    assert_eq!(object.kind(), SyntaxKind::Object);
    let member = object.children().next().unwrap();
    assert_eq!(member.kind(), SyntaxKind::Member);
}

#[test]
fn tree_structure_array() {
    let input = "[1, 2, 3]";
    let root = parse_json(input);

    let array = root.first_child().unwrap();
    assert_eq!(array.kind(), SyntaxKind::Array);
}

#[test]
fn parse_error_on_missing_brace() {
    let result = parse(r#"{"a": 1"#, Dialect::Json);
    assert!(!result.errors.is_empty());
    // Still produces a tree (error recovery)
    let root = SyntaxNode::new_root(result.green_node);
    assert_eq!(root.kind(), SyntaxKind::Root);
}

#[test]
fn parse_error_on_missing_value() {
    let result = parse(r#"{"a": }"#, Dialect::Json);
    assert!(!result.errors.is_empty());
}

#[test]
fn multiline_object_preserves_whitespace() {
    let input = indoc::indoc! {r#"
        {
          "first": 1,
          "second": 2
        }
    "#}
    .trim();
    let root = parse_json(input);
    assert_eq!(root.to_string(), input);
}
