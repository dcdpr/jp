//! Tests for the language resolution rules: which `Format` we end up with given
//! the `--language` flag and an optional filename hint.

use std::path::Path;

use clap::Parser;
use pretty_assertions::assert_eq;

use super::{Cli, Format, Language};
use crate::config::Config;

fn style(args: &[&str]) -> Config {
    Cli::parse_from(std::iter::once("comfort").chain(args.iter().copied())).style()
}

#[test]
fn no_style_flags_set_nothing() {
    // Every setting is left to `.comfort.toml`, so the file isn't overridden
    // by defaults the user never typed.
    assert_eq!(style(&[]), Config::default());
}

#[test]
fn style_flags_set_their_settings() {
    assert_eq!(
        style(&[
            "--max-width",
            "72",
            "--format-markdown",
            "--reference-links",
            "--prune-reference-links",
        ]),
        Config {
            max_width: Some(72),
            format_markdown: Some(true),
            reference_links: Some(true),
            prune_reference_links: Some(true),
        }
    );
}

#[test]
fn no_flags_turn_their_settings_off() {
    assert_eq!(
        style(&[
            "--no-format-markdown",
            "--no-reference-links",
            "--no-prune-reference-links",
        ]),
        Config {
            max_width: None,
            format_markdown: Some(false),
            reference_links: Some(false),
            prune_reference_links: Some(false),
        }
    );
}

#[test]
fn the_last_of_a_flag_and_its_negation_wins() {
    let config = style(&[
        "--format-markdown",
        "--no-format-markdown",
        "--no-reference-links",
        "--reference-links",
    ]);

    assert_eq!(config.format_markdown, Some(false));
    assert_eq!(config.reference_links, Some(true));
}

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
