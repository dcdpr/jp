//! Tests for [`PartialAppConfig::unset`].

use schematic::PartialConfig as _;
use test_log::test;

use super::*;
use crate::providers::mcp::{PartialMcpProviderConfig, PartialStdioConfig};

/// A partial holding one MCP server with two arguments.
fn partial_with_server() -> PartialAppConfig {
    let mut partial = PartialAppConfig::empty();
    partial.providers.mcp.insert(
        "bookworm".to_owned(),
        PartialMcpProviderConfig::Stdio(PartialStdioConfig {
            command: Some("just".into()),
            arguments: Some(vec!["serve".to_owned(), "--verbose".to_owned()]),
            ..PartialStdioConfig::default()
        }),
    );
    partial
}

/// The `arguments` of the `bookworm` server, if the entry is present.
fn arguments(partial: &PartialAppConfig) -> Option<&Vec<String>> {
    let PartialMcpProviderConfig::Stdio(config) = partial.providers.mcp.get("bookworm")?;
    config.arguments.as_ref()
}

#[test]
fn unset_clears_a_scalar_field() {
    let mut partial = PartialAppConfig::empty();
    partial.assistant.name = Some("DevBot".to_owned());

    partial.unset("assistant.name").unwrap();

    assert_eq!(partial.assistant.name, None);
}

/// An appending list has to reach `None`, not `Some([])`: schematic only runs a
/// field's merge strategy when both layers hold a value, so `Some([])` would
/// still append to whatever the next layer brings.
#[test]
fn unset_clears_a_list_to_none() {
    let mut partial = partial_with_server();

    partial
        .unset("providers.mcp.bookworm.arguments")
        .expect("`arguments` is a field");

    assert_eq!(arguments(&partial), None);
    assert!(
        partial.providers.mcp.contains_key("bookworm"),
        "clearing a field leaves the entry that holds it"
    );
}

#[test]
fn unset_removes_a_map_entry() {
    let mut partial = partial_with_server();

    partial.unset("providers.mcp.bookworm").unwrap();

    assert!(partial.providers.mcp.is_empty());
}

#[test]
fn unset_of_an_absent_map_entry_is_a_no_op() {
    let mut partial = partial_with_server();

    partial.unset("providers.mcp.kagi.arguments").unwrap();

    assert_eq!(
        arguments(&partial),
        Some(&vec!["serve".to_owned(), "--verbose".to_owned()]),
        "an unrelated entry is untouched"
    );
}

#[test]
fn unset_reports_an_unknown_path() {
    let mut partial = PartialAppConfig::empty();

    let error = partial.unset("assistant.nope").unwrap_err().to_string();

    assert!(
        error.contains("assistant.nope"),
        "the error names the path: {error}"
    );
}

/// The point of clearing: a cleared field takes the next layer's value whole,
/// where an uncleared one would have appended to it.
#[test]
fn a_cleared_list_takes_the_next_layers_value_verbatim() {
    let mut prev = partial_with_server();

    let mut next = PartialAppConfig::empty();
    next.providers.mcp.insert(
        "bookworm".to_owned(),
        PartialMcpProviderConfig::Stdio(PartialStdioConfig {
            arguments: Some(vec!["serve".to_owned()]),
            ..PartialStdioConfig::default()
        }),
    );

    prev.unset("providers.mcp.bookworm.arguments").unwrap();
    prev.merge(&(), next).unwrap();

    assert_eq!(arguments(&prev), Some(&vec!["serve".to_owned()]));
}

/// Without the clear, the same merge appends — the behavior that makes a
/// removal impossible to express as a delta.
#[test]
fn an_uncleared_list_appends_the_next_layers_value() {
    let mut prev = partial_with_server();

    let mut next = PartialAppConfig::empty();
    next.providers.mcp.insert(
        "bookworm".to_owned(),
        PartialMcpProviderConfig::Stdio(PartialStdioConfig {
            arguments: Some(vec!["serve".to_owned()]),
            ..PartialStdioConfig::default()
        }),
    );

    prev.merge(&(), next).unwrap();

    assert_eq!(
        arguments(&prev),
        Some(&vec![
            "serve".to_owned(),
            "--verbose".to_owned(),
            "serve".to_owned()
        ])
    );
}
