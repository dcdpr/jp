use pretty_assertions::assert_eq;
use serde_json::{Map, json};

use super::env_from_options;

fn options(value: &serde_json::Value) -> Map<String, serde_json::Value> {
    value.as_object().unwrap().clone()
}

/// A redirected root can be granted read without update, and `git status` would
/// otherwise refresh — and so rewrite — the index it was told not to touch.
#[test]
fn every_invocation_drops_gits_optional_locks() {
    assert_eq!(env_from_options(&Map::new()), vec![(
        "GIT_OPTIONAL_LOCKS",
        "0"
    )]);
}

#[test]
fn configured_variables_are_appended_to_the_defaults() {
    let options = options(&json!({"env": {"GIT_CONFIG_GLOBAL": "/dev/null"}}));

    assert_eq!(env_from_options(&options), vec![
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ]);
}

/// The runner applies the pairs in order, so the later entry is the one git
/// sees.
/// A caller that needs the index refreshed can therefore ask for it.
#[test]
fn a_configured_variable_comes_after_the_default_it_replaces() {
    let options = options(&json!({"env": {"GIT_OPTIONAL_LOCKS": "1"}}));

    assert_eq!(env_from_options(&options), vec![
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_OPTIONAL_LOCKS", "1"),
    ]);
}
