use indoc::indoc;
use pretty_assertions::assert_eq;
use schematic::PartialConfig as _;
use serde_json::json;

use super::*;

#[test]
fn run_policy_default_is_ask() {
    assert_eq!(RunPolicy::default(), RunPolicy::Ask);
}

#[test]
fn run_policy_roundtrip() {
    let json = serde_json::to_string(&RunPolicy::Unattended).unwrap();
    assert_eq!(json, "\"unattended\"");
    let parsed: RunPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, RunPolicy::Unattended);
}

#[test]
fn command_plugin_config_from_toml() {
    let toml = indoc! {r#"
        install = true
        run = "unattended"

        [checksum]
        algorithm = "sha256"
        value = "abc123"

        [options]
        web.port = 2000
        web.host = "0.0.0.0"
    "#};

    let partial: PartialCommandPluginConfig = toml::from_str(toml).unwrap();
    assert_eq!(partial.install, Some(true));
    assert_eq!(partial.run, Some(RunPolicy::Unattended));
    assert!(partial.checksum.is_some());

    let opts = partial.options;
    assert_eq!(opts["web"]["port"], json!(2000));
    assert_eq!(opts["web"]["host"], json!("0.0.0.0"));
}

/// Layer `next` over `base` the way the config pipeline does, and resolve.
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
fn command_plugin_config_minimal() {
    let toml = "run = \"deny\"\n";
    let partial: PartialCommandPluginConfig = toml::from_str(toml).unwrap();
    assert_eq!(partial.run, Some(RunPolicy::Deny));
    assert!(partial.install.is_none());
    assert!(partial.checksum.is_none());
    assert!(partial.options.is_empty());
}
