use rowan::Language;

use super::*;

#[test]
fn kind_roundtrip() {
    for raw in 0..=SyntaxKind::Member as u16 {
        let kind = JsonLang::kind_from_raw(rowan::SyntaxKind(raw));
        assert_eq!(JsonLang::kind_to_raw(kind).0, raw);
    }
}

#[test]
fn trivia_classification() {
    assert!(SyntaxKind::Whitespace.is_trivia());
    assert!(SyntaxKind::LineComment.is_trivia());
    assert!(SyntaxKind::BlockComment.is_trivia());
    assert!(!SyntaxKind::String.is_trivia());
    assert!(!SyntaxKind::Comma.is_trivia());
    assert!(!SyntaxKind::Member.is_trivia());
}
