//! Tests for the language resolution rules: which `Format` we end up with given
//! the `--language` flag and an optional filename hint.

use std::path::Path;

use pretty_assertions::assert_eq;

use super::{Format, Language};

#[test]
fn auto_with_rust_extension_resolves_to_rust() {
    assert_eq!(
        Language::Auto.resolve(Some(Path::new("foo.rs"))),
        Format::Rust
    );
}

#[test]
fn auto_with_markdown_extension_resolves_to_markdown() {
    assert_eq!(
        Language::Auto.resolve(Some(Path::new("foo.md"))),
        Format::Markdown
    );
    assert_eq!(
        Language::Auto.resolve(Some(Path::new("foo.markdown"))),
        Format::Markdown
    );
}

#[test]
fn auto_with_unknown_extension_falls_back_to_rust() {
    assert_eq!(
        Language::Auto.resolve(Some(Path::new("foo.txt"))),
        Format::Rust
    );
}

#[test]
fn auto_with_no_filename_hint_falls_back_to_rust() {
    // Stdin without `--stdin-filename`: no extension to detect from.
    assert_eq!(Language::Auto.resolve(None), Format::Rust);
}

#[test]
fn explicit_rust_overrides_markdown_extension() {
    // The pushed-back case: user has rust code in a `.md` file (slides,
    // generated stub, whatever) and forces rust mode.
    assert_eq!(
        Language::Rust.resolve(Some(Path::new("slides.md"))),
        Format::Rust
    );
}

#[test]
fn explicit_markdown_overrides_rust_extension() {
    // Inverse of the above: rare but symmetric.
    assert_eq!(
        Language::Markdown.resolve(Some(Path::new("notes.rs"))),
        Format::Markdown
    );
}

#[test]
fn explicit_language_wins_over_missing_hint() {
    // Stdin with `--language markdown` and no `--stdin-filename`: still
    // markdown.
    assert_eq!(Language::Markdown.resolve(None), Format::Markdown);
    assert_eq!(Language::Rust.resolve(None), Format::Rust);
}
