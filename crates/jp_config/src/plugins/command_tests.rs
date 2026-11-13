use indexmap::IndexMap;
use indoc::indoc;
use pretty_assertions::assert_eq;
use schematic::PartialConfig as _;
use serde_json::json;

use super::*;
use crate::types::map::{MergedMap, MergedMapStrategy};

fn section_layer(toml: &str) -> PartialCommandPluginsConfig {
    toml::from_str(toml).unwrap()
}

fn section(toml: &str) -> CommandPluginsConfig {
    CommandPluginsConfig::from_partial(section_layer(toml).finalize(&()).unwrap(), vec![]).unwrap()
}

fn run_of(layer: &PartialCommandPluginsConfig, name: &str) -> Option<RunPolicy> {
    layer.plugins.get(name).and_then(|plugin| plugin.run)
}

#[test]
fn a_plugin_without_its_own_shutdown_timeout_gets_the_default() {
    let config = section(indoc! {r#"
        ['*']
        shutdown_timeout_secs = 20

        [serve-web]
        run = "allow"
    "#});

    assert_eq!(
        config.shutdown_timeout("serve-web"),
        Duration::from_secs(20)
    );
}

#[test]
fn a_plugins_own_shutdown_timeout_overrides_the_default() {
    let config = section(indoc! {"
        ['*']
        shutdown_timeout_secs = 20

        [serve-chat]
        shutdown_timeout_secs = 90

        [serve-web]
        shutdown_timeout_secs = 0
    "});

    assert_eq!(
        config.shutdown_timeout("serve-chat"),
        Duration::from_secs(90)
    );
    // Explicit 0 disables the grace period rather than falling back to the default.
    assert_eq!(config.shutdown_timeout("serve-web"), Duration::ZERO);
}

#[test]
fn a_plugin_that_is_not_configured_gets_the_default() {
    let config = section(indoc! {"
        [serve-chat]
        shutdown_timeout_secs = 90
    "});

    assert_eq!(config.shutdown_timeout("ticket"), Duration::from_secs(5));
}

#[test]
fn the_defaults_are_not_a_plugin() {
    let config = section(indoc! {"
        ['*']
        shutdown_timeout_secs = 20
    "});

    assert_eq!(config.defaults.shutdown_timeout_secs, 20);
    assert_eq!(config.get("*"), None);
}

#[test]
fn the_default_shutdown_timeout_is_assigned_by_key() {
    let mut layer = PartialCommandPluginsConfig::default();

    layer
        .assign(KvAssignment::try_from_cli("*.shutdown_timeout_secs", "20").unwrap())
        .unwrap();

    assert_eq!(layer.defaults.shutdown_timeout_secs, Some(20));
    assert_eq!(layer.plugins, MergeableMap::default());
}

#[test]
fn an_object_assigns_the_defaults_and_the_plugins_it_names() {
    let mut layer = section_layer(indoc! {r#"
        [ticket]
        run = "deny"
    "#});

    layer
        .assign(
            KvAssignment::try_from_cli(
                ":",
                r#"{"*":{"shutdown_timeout_secs":20},"serve":{"run":"allow"}}"#,
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(layer.defaults.shutdown_timeout_secs, Some(20));
    assert_eq!(run_of(&layer, "serve"), Some(RunPolicy::Allow));
    assert_eq!(run_of(&layer, "ticket"), Some(RunPolicy::Deny));
    assert!(!layer.plugins.contains_key("*"));
}

#[test]
fn the_plugins_map_accepts_a_replace_strategy() {
    let from_toml = section_layer(indoc! {r#"
        strategy = "replace"

        [value.serve]
        run = "allow"
    "#});

    let mut from_cli = PartialCommandPluginsConfig::default();
    from_cli
        .assign(
            KvAssignment::try_from_cli(
                ":",
                r#"{"value":{"serve":{"run":"allow"}},"strategy":"replace"}"#,
            )
            .unwrap(),
        )
        .unwrap();

    let expected = MergeableMap::Merged(MergedMap {
        value: IndexMap::from([("serve".to_owned(), PartialCommandPluginConfig {
            run: Some(RunPolicy::Allow),
            ..Default::default()
        })]),
        strategy: Some(MergedMapStrategy::Replace),
        discard_when_merged: false,
    });

    assert_eq!(from_toml.plugins, expected);
    assert_eq!(from_cli.plugins, expected);
}

#[test]
fn a_later_layer_overrides_the_default_and_keeps_the_plugins_of_earlier_layers() {
    let mut layer = section_layer(indoc! {r#"
        ['*']
        shutdown_timeout_secs = 20

        [serve]
        run = "allow"
    "#});
    let next = section_layer(indoc! {r#"
        ['*']
        shutdown_timeout_secs = 30

        [ticket]
        run = "deny"
    "#});

    layer.merge(&(), next).unwrap();

    assert_eq!(layer.defaults.shutdown_timeout_secs, Some(30));
    assert_eq!(run_of(&layer, "serve"), Some(RunPolicy::Allow));
    assert_eq!(run_of(&layer, "ticket"), Some(RunPolicy::Deny));
}

#[test]
fn a_missing_default_is_filled_in_and_a_set_one_is_kept() {
    let below = section_layer(indoc! {r#"
        ['*']
        shutdown_timeout_secs = 20

        [ticket]
        run = "deny"
    "#});

    let unset = section_layer(indoc! {r#"
        [serve]
        run = "allow"
    "#})
    .fill_from(below.clone());

    assert_eq!(unset.defaults.shutdown_timeout_secs, Some(20));
    assert_eq!(run_of(&unset, "serve"), Some(RunPolicy::Allow));
    assert_eq!(run_of(&unset, "ticket"), Some(RunPolicy::Deny));

    let set = section_layer(indoc! {"
        ['*']
        shutdown_timeout_secs = 30
    "})
    .fill_from(below);

    assert_eq!(set.defaults.shutdown_timeout_secs, Some(30));
}

#[test]
fn a_changed_default_is_all_its_delta_carries() {
    let prev = section_layer(indoc! {r#"
        ['*']
        shutdown_timeout_secs = 20

        [serve]
        run = "allow"
    "#});
    let next = section_layer(indoc! {r#"
        ['*']
        shutdown_timeout_secs = 30

        [serve]
        run = "allow"
    "#});

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next, "plugins.command", &mut unsets);

    assert_eq!(delta.defaults.shutdown_timeout_secs, Some(30));
    assert_eq!(delta.plugins, MergeableMap::default());
    assert_eq!(unsets, Vec::<String>::new());

    assert_eq!(
        prev.delta(prev.clone()),
        PartialCommandPluginsConfig::default()
    );
}

#[test]
fn a_dropped_plugin_is_cleared_at_its_path_under_the_section() {
    let prev = section_layer(indoc! {r#"
        [serve]
        run = "allow"

        [ticket]
        run = "deny"
    "#});
    let next = section_layer(indoc! {r#"
        [ticket]
        run = "deny"
    "#});

    let mut unsets = Vec::new();
    let delta = prev.delta_with_unsets(next, "plugins.command", &mut unsets);

    assert_eq!(unsets, ["plugins.command.serve"]);
    assert!(!delta.plugins.contains_key("serve"));
}

#[test]
fn the_defaults_and_a_plugins_own_value_survive_a_round_trip() {
    let config = section(indoc! {"
        ['*']
        shutdown_timeout_secs = 20

        [serve]
        shutdown_timeout_secs = 90
    "});

    let partial = config.to_partial();

    assert_eq!(partial.defaults.shutdown_timeout_secs, Some(20));
    assert_eq!(
        partial
            .plugins
            .get("serve")
            .and_then(|plugin| plugin.shutdown_timeout_secs),
        Some(90)
    );
}

#[test]
fn the_default_run_policy_is_ask_unless_a_layer_sets_it() {
    assert_eq!(section("").defaults.run, RunPolicy::Ask);

    let config = section(indoc! {r#"
        ['*']
        run = "deny"
    "#});

    assert_eq!(config.defaults.run, RunPolicy::Deny);
    // Setting one default leaves the other at its built-in value.
    assert_eq!(config.defaults.shutdown_timeout_secs, 5);
}

#[test]
fn the_default_run_policy_is_assigned_by_key() {
    let mut layer = PartialCommandPluginsConfig::default();

    layer
        .assign(KvAssignment::try_from_cli("*.run", "deny").unwrap())
        .unwrap();

    assert_eq!(layer.defaults.run, Some(RunPolicy::Deny));
    assert_eq!(layer.plugins, MergeableMap::default());
}

#[test]
fn a_later_layer_overrides_the_default_run_policy_and_keeps_a_plugins_own() {
    let mut layer = section_layer(indoc! {r#"
        ['*']
        run = "allow"

        [serve]
        run = "ask"
    "#});
    let next = section_layer(indoc! {r#"
        ['*']
        run = "deny"
    "#});

    layer.merge(&(), next).unwrap();

    assert_eq!(layer.defaults.run, Some(RunPolicy::Deny));
    assert_eq!(run_of(&layer, "serve"), Some(RunPolicy::Ask));
}

#[test]
fn a_missing_default_run_policy_is_filled_in_and_a_set_one_is_kept() {
    let below = section_layer(indoc! {r#"
        ['*']
        run = "deny"
    "#});

    let unset = section_layer("").fill_from(below.clone());
    assert_eq!(unset.defaults.run, Some(RunPolicy::Deny));

    let set = section_layer(indoc! {r#"
        ['*']
        run = "allow"
    "#})
    .fill_from(below);
    assert_eq!(set.defaults.run, Some(RunPolicy::Allow));
}

#[test]
fn a_changed_default_run_policy_is_carried_by_its_delta_and_a_round_trip() {
    let prev = section_layer(indoc! {r#"
        ['*']
        run = "ask"
    "#});
    let next = section_layer(indoc! {r#"
        ['*']
        run = "deny"
    "#});

    assert_eq!(prev.delta(prev.clone()).defaults.run, None);
    assert_eq!(prev.delta(next).defaults.run, Some(RunPolicy::Deny));

    let config = section(indoc! {r#"
        ['*']
        run = "deny"
    "#});
    assert_eq!(config.to_partial().defaults.run, Some(RunPolicy::Deny));

    // `to_partial` emits the resolved default so conversations preserve the
    // default they were created with.
    assert_eq!(section("").to_partial().defaults.run, Some(RunPolicy::Ask));
}

#[test]
fn run_policy_default_is_ask() {
    assert_eq!(RunPolicy::default(), RunPolicy::Ask);
}

#[test]
fn run_policy_roundtrip() {
    let json = serde_json::to_string(&RunPolicy::Allow).unwrap();
    assert_eq!(json, "\"allow\"");
    let parsed: RunPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, RunPolicy::Allow);
}

#[test]
fn unattended_reads_as_allow() {
    let partial: PartialCommandPluginConfig = toml::from_str(r#"run = "unattended""#).unwrap();
    assert_eq!(partial.run, Some(RunPolicy::Allow));
}

#[test]
fn command_plugin_config_from_toml() {
    let toml = indoc! {r#"
        run = "allow"

        [checksum]
        algorithm = "sha256"
        value = "abc123"

        [options]
        web.port = 2000
        web.host = "0.0.0.0"
    "#};

    let partial: PartialCommandPluginConfig = toml::from_str(toml).unwrap();
    assert_eq!(partial.run, Some(RunPolicy::Allow));
    assert!(partial.checksum.is_some());

    let opts = partial.options;
    assert_eq!(opts["web"]["port"], json!(2000));
    assert_eq!(opts["web"]["host"], json!("0.0.0.0"));
}

fn layer(base: &str, next: &str) -> CommandPluginConfig {
    let mut partial: PartialCommandPluginConfig = toml::from_str(base).unwrap();
    let next: PartialCommandPluginConfig = toml::from_str(next).unwrap();
    partial.merge(&(), next).unwrap();

    CommandPluginConfig::from_partial(partial.finalize(&()).unwrap(), vec![]).unwrap()
}

#[test]
fn a_later_layer_keeps_the_options_it_does_not_name() {
    let config = layer(
        indoc! {r#"
            [options]
            dir = "docs/ticket"
            assistant = "jp"
        "#},
        indoc! {r#"
            [options]
            assistant = "me"
        "#},
    );

    assert_eq!(
        serde_json::to_value(&config.options).unwrap(),
        json!({ "dir": "docs/ticket", "assistant": "me" })
    );
}

#[test]
fn a_later_layer_merges_nested_options_recursively() {
    let config = layer(
        indoc! {"
            [options.web]
            port = 2000
        "},
        indoc! {r#"
            [options.web]
            host = "0.0.0.0"
        "#},
    );

    assert_eq!(
        serde_json::to_value(&config.options).unwrap(),
        json!({ "web": { "port": 2000, "host": "0.0.0.0" } })
    );
}

#[test]
fn an_empty_list_in_the_options_is_kept() {
    let single = layer("", indoc! {"
        [options.labels.rfd]
        values = []
    "});
    assert_eq!(
        serde_json::to_value(&single.options).unwrap(),
        json!({ "labels": { "rfd": { "values": [] } } })
    );

    let layered = layer(
        indoc! {r#"
            [options.labels.rfd]
            values = ["042"]
        "#},
        indoc! {"
            [options.labels.rfd]
            values = []
        "},
    );
    assert_eq!(
        serde_json::to_value(&layered.options).unwrap(),
        json!({ "labels": { "rfd": { "values": [] } } })
    );
}

#[test]
fn a_replace_strategy_drops_the_options_of_earlier_layers() {
    let config = layer(
        indoc! {r#"
            [options]
            dir = "docs/ticket"
            assistant = "jp"
        "#},
        indoc! {r#"
            [options]
            value = { assistant = "me" }
            strategy = "replace"
        "#},
    );

    assert_eq!(
        serde_json::to_value(&config.options).unwrap(),
        json!({ "assistant": "me" })
    );
}

#[test]
fn a_plugin_names_its_own_shutdown_timeout() {
    let partial: PartialCommandPluginConfig = toml::from_str("shutdown_timeout_secs = 90").unwrap();
    assert_eq!(partial.shutdown_timeout_secs, Some(90));

    let config = layer("shutdown_timeout_secs = 90", "");
    assert_eq!(config.shutdown_timeout_secs, Some(90));
}

#[test]
fn a_plugins_shutdown_timeout_is_assigned_by_key() {
    let mut partial = PartialCommandPluginConfig::default();

    partial
        .assign(KvAssignment::try_from_cli("shutdown_timeout_secs", "90").unwrap())
        .unwrap();

    assert_eq!(partial.shutdown_timeout_secs, Some(90));
}

#[test]
fn a_plugins_shutdown_timeout_survives_a_delta_and_a_round_trip() {
    let prev: PartialCommandPluginConfig = toml::from_str("shutdown_timeout_secs = 90").unwrap();
    let same: PartialCommandPluginConfig = toml::from_str("shutdown_timeout_secs = 90").unwrap();
    let next: PartialCommandPluginConfig = toml::from_str("shutdown_timeout_secs = 30").unwrap();

    assert_eq!(prev.delta(same).shutdown_timeout_secs, None);
    assert_eq!(prev.delta(next).shutdown_timeout_secs, Some(30));

    let config = layer("shutdown_timeout_secs = 90", "");
    assert_eq!(config.to_partial().shutdown_timeout_secs, Some(90));
}

#[test]
fn command_plugin_config_minimal() {
    let toml = "run = \"deny\"\n";
    let partial: PartialCommandPluginConfig = toml::from_str(toml).unwrap();
    assert_eq!(partial.run, Some(RunPolicy::Deny));
    assert!(partial.checksum.is_none());
    assert_eq!(partial.shutdown_timeout_secs, None);
    assert!(partial.options.is_empty());
}
