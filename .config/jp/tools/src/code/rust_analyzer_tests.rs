use serde_json::json;

use super::*;

#[test]
fn reads_the_parts_of_an_impl() {
    assert_eq!(impl_parts("impl Stream"), Some((None, "Stream")));
    assert_eq!(
        impl_parts("impl Render for Stream"),
        Some((Some("Render"), "Stream"))
    );
    assert_eq!(
        impl_parts("impl<T: Clone> From<Vec<T>> for Wrapper<T>"),
        Some((Some("From<Vec<T>>"), "Wrapper<T>"))
    );
    assert_eq!(impl_parts("Stream"), None);
}

#[test]
fn names_an_impl_through_its_self_type() {
    assert_eq!(base_ident("Wrapper<T>"), "Wrapper");
    assert_eq!(base_ident("&mut crate::stream::Stream"), "Stream");
    assert_eq!(base_ident("dyn Render"), "Render");
}

#[test]
fn recognizes_test_files_by_path() {
    assert!(is_test_path("crates/jp_cli/src/editor_tests.rs"));
    assert!(is_test_path("crates/jp_cli/tests/cli.rs"));
    assert!(is_test_path("crates/jp_llm/src/test.rs"));
    assert!(!is_test_path("crates/jp_cli/src/editor.rs"));
    assert!(!is_test_path("crates/jp_test/src/lib.rs"));
}

#[test]
fn reads_doc_comment_text() {
    assert_eq!(doc_text("/// A stream."), Some("A stream."));
    assert_eq!(doc_text("///"), Some(""));
    assert_eq!(doc_text("//! Inner."), None);
    assert!(is_preamble("#[derive(Debug)]"));
    assert!(!is_preamble("pub fn total() {}"));
}

#[test]
fn reads_container_and_signature_from_a_hover() {
    let hover = "\n```rust\nfixture::stream\n```\n\n```rust\npub struct Stream {\n    pub items: \
                 Vec<u32>,\n}\n```\n\n---\n\nA stream of numbers.";

    assert_eq!(parse_hover(hover, 12), HoverParts {
        container: Some("fixture::stream".to_owned()),
        signature: vec![
            "pub struct Stream {".to_owned(),
            "    pub items: Vec<u32>,".to_owned(),
            "}".to_owned(),
        ],
    });
}

#[test]
fn stops_at_the_first_rule_of_a_hover() {
    // A trait's hover puts its dyn-compatibility between the signature and the
    // doc comment, with a code block of its own in some versions.
    let hover = "\n```rust\nfixture\n```\n\n```rust\npub trait Render\n```\n\n---\n\nIs \
                 dyn-compatible\n\n```rust\nnot a signature\n```\n\n---\n\nRenders a value.";

    assert_eq!(parse_hover(hover, 12), HoverParts {
        container: Some("fixture".to_owned()),
        signature: vec!["pub trait Render".to_owned()],
    });
}

#[test]
fn a_hover_of_a_field_and_of_a_local() {
    let field = "\n```rust\nfixture::stream::Stream\n```\n\n```rust\npub config: String\n```";
    let local = "```rust\nlet total: u32\n```";

    assert_eq!(parse_hover(field, 12), HoverParts {
        container: Some("fixture::stream::Stream".to_owned()),
        signature: vec!["pub config: String".to_owned()],
    });
    assert_eq!(parse_hover(local, 12), HoverParts {
        container: None,
        signature: vec!["let total: u32".to_owned()],
    });
}

#[test]
fn the_workspace_is_ready_once_the_server_is_quiescent() {
    let status = |quiescent: bool, health: &str| json!({ "health": health, "quiescent": quiescent, "message": "cargo metadata failed" });

    assert_eq!(server_status(&status(false, "ok")), None);
    assert_eq!(server_status(&status(true, "warning")), Some(Ok(())));
    assert_eq!(
        server_status(&status(true, "error")),
        Some(Err("cargo metadata failed".to_owned()))
    );
    assert_eq!(server_status(&json!({ "other": 1 })), None);
}
