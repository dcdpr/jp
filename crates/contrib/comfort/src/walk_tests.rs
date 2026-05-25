//! Tests for the pure parts of workspace walking: the include/exclude predicate
//! and the unknown-package validation.
//! The actual file walking and `cargo_metadata` invocation are covered by
//! end-to-end integration tests via the binary, not unit-tested here.

use std::path::Path;

use pretty_assertions::assert_eq;

use super::{matches_language, should_include, validate_package_names};
use crate::{Error, cli::Language};

fn names(vs: &[&str]) -> Vec<String> {
    vs.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn empty_include_empty_exclude_keeps_every_package() {
    assert!(should_include("foo", &[], &[]));
    assert!(should_include("bar", &[], &[]));
}

#[test]
fn explicit_include_restricts_to_listed_packages() {
    let include = names(&["foo", "bar"]);
    assert!(should_include("foo", &include, &[]));
    assert!(should_include("bar", &include, &[]));
    assert!(!should_include("baz", &include, &[]));
}

#[test]
fn exclude_alone_keeps_unlisted_packages() {
    let exclude = names(&["baz"]);
    assert!(should_include("foo", &[], &exclude));
    assert!(!should_include("baz", &[], &exclude));
}

#[test]
fn exclude_takes_precedence_over_include() {
    // A package both included AND excluded is excluded. This matches
    // `cargo check --workspace --exclude foo`-style semantics.
    let include = names(&["foo", "bar"]);
    let exclude = names(&["foo"]);
    assert!(!should_include("foo", &include, &exclude));
    assert!(should_include("bar", &include, &exclude));
}

#[test]
fn validate_succeeds_when_every_name_matches() {
    let available = ["foo", "bar", "baz"];
    let names = names(&["bar", "baz"]);
    assert!(validate_package_names(&available, &names).is_ok());
}

#[test]
fn validate_succeeds_on_empty_input() {
    let available = ["foo"];
    assert!(validate_package_names(&available, &[]).is_ok());
}

#[test]
fn language_auto_accepts_both_rust_and_markdown() {
    assert!(matches_language(Path::new("foo.rs"), Language::Auto));
    assert!(matches_language(Path::new("foo.md"), Language::Auto));
    assert!(matches_language(Path::new("foo.markdown"), Language::Auto));
    assert!(!matches_language(Path::new("foo.txt"), Language::Auto));
    assert!(!matches_language(Path::new("Cargo.toml"), Language::Auto));
}

#[test]
fn language_rust_filters_to_rs_only() {
    assert!(matches_language(Path::new("foo.rs"), Language::Rust));
    assert!(!matches_language(Path::new("foo.md"), Language::Rust));
    assert!(!matches_language(Path::new("foo.markdown"), Language::Rust));
}

#[test]
fn language_markdown_filters_to_md_and_markdown() {
    assert!(matches_language(Path::new("foo.md"), Language::Markdown));
    assert!(matches_language(
        Path::new("foo.markdown"),
        Language::Markdown
    ));
    assert!(!matches_language(Path::new("foo.rs"), Language::Markdown));
}

#[test]
fn language_filter_skips_files_without_extension() {
    assert!(!matches_language(Path::new("README"), Language::Auto));
    assert!(!matches_language(Path::new("Makefile"), Language::Rust));
    assert!(!matches_language(Path::new("LICENSE"), Language::Markdown));
}

#[test]
fn validate_returns_first_unknown_name() {
    let available = ["foo", "bar"];
    let lookup = names(&["bar", "ghost"]);
    match validate_package_names(&available, &lookup) {
        Err(Error::UnknownPackage(name)) => assert_eq!(name, "ghost"),
        other => panic!("expected UnknownPackage, got {other:?}"),
    }
}
