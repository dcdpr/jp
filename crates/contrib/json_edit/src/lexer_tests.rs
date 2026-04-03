use pretty_assertions::assert_eq;

use super::*;

fn json(input: &str) -> Vec<(SyntaxKind, &str)> {
    lex(input, Dialect::Json)
}

fn json5(input: &str) -> Vec<(SyntaxKind, &str)> {
    lex(input, Dialect::Json5)
}

#[test]
fn empty_input() {
    assert_eq!(json(""), vec![]);
}

#[test]
fn punctuation() {
    let tokens = json("{}[]:,");
    assert_eq!(tokens, vec![
        (SyntaxKind::LBrace, "{"),
        (SyntaxKind::RBrace, "}"),
        (SyntaxKind::LBracket, "["),
        (SyntaxKind::RBracket, "]"),
        (SyntaxKind::Colon, ":"),
        (SyntaxKind::Comma, ","),
    ]);
}

#[test]
fn whitespace() {
    let tokens = json("  \t\n\r  ");
    assert_eq!(tokens, vec![(SyntaxKind::Whitespace, "  \t\n\r  ")]);
}

#[test]
fn keywords() {
    let tokens = json("true false null");
    assert_eq!(tokens, vec![
        (SyntaxKind::TrueKw, "true"),
        (SyntaxKind::Whitespace, " "),
        (SyntaxKind::FalseKw, "false"),
        (SyntaxKind::Whitespace, " "),
        (SyntaxKind::NullKw, "null"),
    ]);
}

#[test]
fn keywords_not_prefix_matched() {
    // "truex" should not match as TrueKw + "x"
    let tokens = json5("truex");
    assert_eq!(tokens, vec![(SyntaxKind::Ident, "truex")]);
}

#[test]
fn double_quoted_string() {
    let tokens = json(r#""hello world""#);
    assert_eq!(tokens, vec![(SyntaxKind::String, r#""hello world""#)]);
}

#[test]
fn string_with_escapes() {
    let tokens = json(r#""line\nbreak""#);
    assert_eq!(tokens, vec![(SyntaxKind::String, r#""line\nbreak""#)]);
}

#[test]
fn string_with_unicode_escape() {
    let tokens = json(r#""\u0041""#);
    assert_eq!(tokens, vec![(SyntaxKind::String, r#""\u0041""#)]);
}

#[test]
fn single_quoted_string_json5() {
    let tokens = json5("'hello'");
    assert_eq!(tokens, vec![(SyntaxKind::String, "'hello'")]);
}

#[test]
fn integer() {
    let tokens = json("42");
    assert_eq!(tokens, vec![(SyntaxKind::Number, "42")]);
}

#[test]
fn negative_number() {
    let tokens = json("-3.14");
    assert_eq!(tokens, vec![(SyntaxKind::Number, "-3.14")]);
}

#[test]
fn number_with_exponent() {
    let tokens = json("1.5e10");
    assert_eq!(tokens, vec![(SyntaxKind::Number, "1.5e10")]);
}

#[test]
fn hex_number_json5() {
    let tokens = json5("0xCAFE");
    assert_eq!(tokens, vec![(SyntaxKind::Number, "0xCAFE")]);
}

#[test]
fn leading_dot_json5() {
    let tokens = json5(".5");
    assert_eq!(tokens, vec![(SyntaxKind::Number, ".5")]);
}

#[test]
fn trailing_dot_json5() {
    let tokens = json5("5.");
    assert_eq!(tokens, vec![(SyntaxKind::Number, "5.")]);
}

#[test]
fn positive_number_json5() {
    let tokens = json5("+42");
    assert_eq!(tokens, vec![(SyntaxKind::Number, "+42")]);
}

#[test]
fn line_comment_json5() {
    let tokens = json5("// comment\n42");
    assert_eq!(tokens, vec![
        (SyntaxKind::LineComment, "// comment"),
        (SyntaxKind::Whitespace, "\n"),
        (SyntaxKind::Number, "42"),
    ]);
}

#[test]
fn block_comment_json5() {
    let tokens = json5("/* block */42");
    assert_eq!(tokens, vec![
        (SyntaxKind::BlockComment, "/* block */"),
        (SyntaxKind::Number, "42"),
    ]);
}

#[test]
fn unquoted_identifier_json5() {
    let tokens = json5("myKey");
    assert_eq!(tokens, vec![(SyntaxKind::Ident, "myKey")]);
}

#[test]
fn identifier_with_dollar_json5() {
    let tokens = json5("$key_1");
    assert_eq!(tokens, vec![(SyntaxKind::Ident, "$key_1")]);
}

#[test]
fn full_json_object() {
    let input = r#"{"a": 1, "b": true}"#;
    let tokens = json(input);
    let kinds: Vec<_> = tokens.iter().map(|(k, _)| *k).collect();
    assert_eq!(kinds, vec![
        SyntaxKind::LBrace,
        SyntaxKind::String,
        SyntaxKind::Colon,
        SyntaxKind::Whitespace,
        SyntaxKind::Number,
        SyntaxKind::Comma,
        SyntaxKind::Whitespace,
        SyntaxKind::String,
        SyntaxKind::Colon,
        SyntaxKind::Whitespace,
        SyntaxKind::TrueKw,
        SyntaxKind::RBrace,
    ]);
    // Lossless: concatenation reproduces input
    let text: String = tokens.iter().map(|(_, s)| *s).collect();
    assert_eq!(text, input);
}

#[test]
fn lossless_roundtrip_json5() {
    let input = "{\n  // a comment\n  key: 'val',\n  num: +.5,\n}";
    let tokens = json5(input);
    let text: String = tokens.iter().map(|(_, s)| *s).collect();
    assert_eq!(text, input);
}

#[test]
fn error_token_for_unexpected_char() {
    let tokens = json("@");
    assert_eq!(tokens, vec![(SyntaxKind::Error, "@")]);
}
