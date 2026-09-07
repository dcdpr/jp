use indexmap::IndexMap;
use test_log::test;

use super::*;
use crate::{
    providers::mcp::{PartialMcpProviderConfig, PartialStdioConfig},
    types::vec::{MergeableVec, MergedVec, MergedVecStrategy},
};

/// A server entry with `arguments` set and every other field unset.
fn server(arguments: &[&str]) -> PartialMcpProviderConfig {
    PartialMcpProviderConfig::Stdio(PartialStdioConfig {
        command: Some("serve".into()),
        arguments: Some(
            arguments
                .iter()
                .map(|a| (*a).to_owned())
                .collect::<Vec<_>>()
                .into(),
        ),
        ..PartialStdioConfig::default()
    })
}

/// A one-server map, keyed as `kagi`.
fn map(arguments: &[&str]) -> IndexMap<String, PartialMcpProviderConfig> {
    let mut map = IndexMap::new();
    map.insert("kagi".to_owned(), server(arguments));
    map
}

/// A removed map entry is reported, since merging cannot take a key away.
#[test]
fn map_delta_reports_a_removed_entry() {
    let prev = map(&["--a"]);
    let next = IndexMap::new();
    let mut unsets = Vec::new();

    let delta = delta_map_with_unsets("providers.mcp", &prev, next, &mut unsets);

    assert!(delta.is_empty(), "nothing to merge for a removed entry");
    assert_eq!(unsets, ["providers.mcp.kagi"]);
}

/// An entry both maps hold is not reported, only diffed.
#[test]
fn map_delta_does_not_report_a_surviving_entry() {
    let prev = map(&["--a"]);
    let next = map(&["--a", "--b"]);
    let mut unsets = Vec::new();

    let delta = delta_map_with_unsets("providers.mcp", &prev, next, &mut unsets);

    assert_eq!(delta.len(), 1);
    assert!(
        unsets.is_empty(),
        "the entry survives, so nothing is cleared"
    );
}

/// The `arguments` of a server entry, for asserting on a computed delta.
fn arguments(entry: &PartialMcpProviderConfig) -> Option<&Vec<String>> {
    let PartialMcpProviderConfig::Stdio(config) = entry;
    config.arguments.as_deref()
}

/// A list the fold appends, holding `values`.
fn appended(values: &[&str]) -> MergeableVec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

/// A list the fold replaces, holding `values`.
fn replaced(values: &[&str]) -> MergeableVec<String> {
    MergeableVec::Merged(MergedVec {
        value: values.iter().map(|v| (*v).to_owned()).collect(),
        strategy: Some(MergedVecStrategy::Replace),
        dedup: None,
        discard_when_merged: false,
    })
}

#[test]
fn vec_delta_appends_the_added_elements() {
    let prev = MergeableVec::from(vec!["--a".to_owned()]);

    assert_eq!(
        delta_opt_mergeable_vec(Some(&prev), Some(appended(&["--a", "--b"]))),
        Some(appended(&["--b"]))
    );
}

/// The first element added to an empty list is still an addition.
#[test]
fn vec_delta_appends_the_first_added_element() {
    let prev = MergeableVec::from(Vec::<String>::new());

    assert_eq!(
        delta_opt_mergeable_vec(Some(&prev), Some(appended(&["--a"]))),
        Some(appended(&["--a"]))
    );
}

#[test]
fn unchanged_vec_has_no_delta() {
    let prev = MergeableVec::from(vec!["--a".to_owned()]);

    assert_eq!(
        delta_opt_mergeable_vec(Some(&prev), Some(appended(&["--a"]))),
        None
    );
}

/// Appending cannot take an element away, so a removal replaces the list.
#[test]
fn removed_vec_element_replaces_the_list() {
    let prev = MergeableVec::from(vec!["--a".to_owned(), "--b".to_owned()]);

    assert_eq!(
        delta_opt_mergeable_vec(Some(&prev), Some(appended(&["--a"]))),
        Some(replaced(&["--a"]))
    );
}

/// Order is part of the value, so a reorder replaces the list too.
#[test]
fn reordered_vec_replaces_the_list() {
    let prev = MergeableVec::from(vec!["--a".to_owned(), "--b".to_owned()]);

    assert_eq!(
        delta_opt_mergeable_vec(Some(&prev), Some(appended(&["--b", "--a"]))),
        Some(replaced(&["--b", "--a"]))
    );
}

/// A one-server config, keyed as `kagi`.
fn config_with_server(arguments: &[&str]) -> crate::PartialAppConfig {
    let mut partial = crate::PartialAppConfig::empty();
    partial
        .providers
        .mcp
        .insert("kagi".to_owned(), server(arguments));
    partial
}

/// A change appending can reach reports no path, and the delta is the tail.
#[test]
fn an_appended_argument_reports_no_path() {
    let prev = config_with_server(&["--a"]);
    let next = config_with_server(&["--a", "--b"]);

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next, "", &mut unsets);

    assert!(unsets.is_empty());
    assert_eq!(
        arguments(&delta.providers.mcp["kagi"]),
        Some(&vec!["--b".to_owned()])
    );
}

/// A change appending cannot reach carries the whole list with `replace`.
///
/// No path is reported: the field states the strategy itself, so the fold has
/// nothing to clear first.
#[test]
fn a_dropped_argument_is_recorded_as_a_replacement() {
    let prev = config_with_server(&["--a", "--b"]);
    let next = config_with_server(&["--a"]);

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next, "", &mut unsets);

    assert!(unsets.is_empty(), "nothing to clear: {unsets:?}");
    assert_eq!(
        arguments(&delta.providers.mcp["kagi"]),
        Some(&vec!["--a".to_owned()])
    );
}

/// Reordering is not an extension either, so it replaces too.
#[test]
fn a_reordered_argument_list_is_recorded_as_a_replacement() {
    let prev = config_with_server(&["--a", "--b"]);
    let next = config_with_server(&["--b", "--a"]);

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next, "", &mut unsets);

    assert!(unsets.is_empty(), "nothing to clear: {unsets:?}");
    assert_eq!(
        arguments(&delta.providers.mcp["kagi"]),
        Some(&vec!["--b".to_owned(), "--a".to_owned()])
    );
}

/// The report reaches a field nested several levels below the root.
#[test]
fn a_dropped_beta_header_is_recorded_as_a_replacement() {
    use crate::types::vec::{MergedVec, MergedVecStrategy};

    let headers = |values: &[&str]| {
        let mut partial = crate::PartialAppConfig::empty();
        partial.providers.llm.anthropic.beta_headers =
            Some(values.iter().map(|v| (*v).to_owned()).collect());
        partial
    };

    let prev = headers(&["one", "two"]);
    let next = headers(&["one"]);

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next, "", &mut unsets);

    assert!(
        unsets.is_empty(),
        "the field says `replace` itself, so no path needs reporting: {unsets:?}"
    );
    assert_eq!(
        delta.providers.llm.anthropic.beta_headers,
        Some(MergeableVec::Merged(MergedVec {
            value: vec!["one".to_owned()],
            strategy: Some(MergedVecStrategy::Replace),
            dedup: None,
            discard_when_merged: false,
        }))
    );
}

/// A dropped stop word is recorded wherever the parameters are reached from.
///
/// The list carries its own strategy, so each site records a replacement and
/// none needs a path reported.
#[test]
fn a_dropped_stop_word_is_recorded_at_every_site() {
    let words = |values: &[&str]| -> Option<MergeableVec<String>> {
        Some(values.iter().map(|v| (*v).to_owned()).collect())
    };

    let mut prev = crate::PartialAppConfig::empty();
    prev.assistant.model.parameters.stop_words = words(&["halt", "stop"]);
    prev.style.reasoning.summary_model = Some(crate::model::PartialModelConfig {
        parameters: crate::model::parameters::PartialParametersConfig {
            stop_words: words(&["halt", "stop"]),
            ..Default::default()
        },
        ..Default::default()
    });

    let mut next = prev.clone();
    next.assistant.model.parameters.stop_words = words(&["halt"]);
    if let Some(model) = next.style.reasoning.summary_model.as_mut() {
        model.parameters.stop_words = words(&["halt"]);
    }

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next, "", &mut unsets);

    let replaced_with = |values: &[&str]| {
        Some(MergeableVec::Merged(MergedVec {
            value: values.iter().map(|v| (*v).to_owned()).collect(),
            strategy: Some(MergedVecStrategy::Replace),
            dedup: None,
            discard_when_merged: false,
        }))
    };

    assert!(unsets.is_empty(), "nothing to clear: {unsets:?}");
    assert_eq!(
        delta.assistant.model.parameters.stop_words,
        replaced_with(&["halt"])
    );
    assert_eq!(
        delta
            .style
            .reasoning
            .summary_model
            .as_ref()
            .map(|model| model.parameters.stop_words.clone()),
        Some(replaced_with(&["halt"])),
        "the second site records its own replacement"
    );
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

/// An entry whose delta carries nothing is left out entirely.
///
/// Keeping it would hand the caller a map with one entry holding nothing, which
/// reads as a change to every emptiness check upstream.
/// A stdio entry no longer reaches that state through its `arguments`, which
/// can now say `replace`, so the case is built directly.
#[test]
fn map_delta_drops_an_entry_whose_delta_is_empty() {
    let entry = |command: &str| -> IndexMap<String, PartialMcpProviderConfig> {
        let mut map = IndexMap::new();
        map.insert(
            "kagi".to_owned(),
            PartialMcpProviderConfig::Stdio(PartialStdioConfig {
                command: Some(command.into()),
                ..PartialStdioConfig::default()
            }),
        );
        map
    };

    // Equal entries are dropped by the equality check ahead of the delta.
    assert!(delta_map(&entry("serve"), entry("serve")).is_empty());

    // A differing entry contributes only what changed.
    let delta = delta_map(&entry("serve"), entry("other"));
    assert_eq!(delta.len(), 1);
}
