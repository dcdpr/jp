use indexmap::IndexMap;
use test_log::test;

use super::*;
use crate::providers::mcp::{PartialMcpProviderConfig, PartialStdioConfig};

/// A server entry with `arguments` set and every other field unset.
fn server(arguments: &[&str]) -> PartialMcpProviderConfig {
    PartialMcpProviderConfig::Stdio(PartialStdioConfig {
        command: Some("serve".into()),
        arguments: Some(arguments.iter().map(|a| (*a).to_owned()).collect()),
        ..PartialStdioConfig::default()
    })
}

/// A one-server map, keyed as `kagi`.
fn map(arguments: &[&str]) -> IndexMap<String, PartialMcpProviderConfig> {
    let mut map = IndexMap::new();
    map.insert("kagi".to_owned(), server(arguments));
    map
}

/// The `arguments` of a server entry, for asserting on a computed delta.
fn arguments(entry: &PartialMcpProviderConfig) -> Option<&Vec<String>> {
    let PartialMcpProviderConfig::Stdio(config) = entry;
    config.arguments.as_ref()
}

#[test]
fn vec_delta_holds_the_added_elements() {
    let prev = vec!["--a".to_owned()];
    let next = vec!["--a".to_owned(), "--b".to_owned()];

    assert_eq!(
        delta_opt_vec(Some(&prev), Some(next)),
        Some(vec!["--b".to_owned()])
    );
}

/// The first element added to an empty vector is still an addition.
#[test]
fn vec_delta_holds_the_first_added_element() {
    let prev = vec![];
    let next = vec!["--a".to_owned()];

    assert_eq!(
        delta_opt_vec(Some(&prev), Some(next)),
        Some(vec!["--a".to_owned()])
    );
}

#[test]
fn unchanged_vec_has_no_delta() {
    let prev = vec!["--a".to_owned()];
    let next = vec!["--a".to_owned()];

    assert_eq!(delta_opt_vec(Some(&prev), Some(next)), None);
}

/// Appending cannot take an element away, so a removal has no delta to record.
#[test]
fn removed_vec_element_has_no_delta() {
    let prev = vec!["--a".to_owned(), "--b".to_owned()];
    let next = vec!["--a".to_owned()];

    assert_eq!(delta_opt_vec(Some(&prev), Some(next)), None);
}

#[test]
fn map_delta_keeps_an_entry_only_next_has() {
    let prev = IndexMap::new();
    let next = map(&["--a"]);

    assert_eq!(delta_map(&prev, next.clone()), next);
}

#[test]
fn map_delta_keeps_the_changed_fields_of_an_entry() {
    let prev = map(&["--a"]);
    let next = map(&["--a", "--b"]);

    let delta = delta_map(&prev, next);

    assert_eq!(delta.len(), 1);
    assert_eq!(arguments(&delta["kagi"]), Some(&vec!["--b".to_owned()]));
}

/// An entry that differs but has no expressible delta is left out entirely.
///
/// Keeping it would hand the caller a map with one entry holding nothing, which
/// reads as a change to every emptiness check upstream.
#[test]
fn map_delta_drops_an_entry_whose_delta_is_empty() {
    let prev = map(&["--a", "--b"]);
    let next = map(&["--a"]);

    assert!(delta_map(&prev, next).is_empty());
}
