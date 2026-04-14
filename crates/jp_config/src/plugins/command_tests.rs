use indoc::indoc;
use pretty_assertions::assert_eq;
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

    let opts = partial.options.unwrap();
    assert_eq!(opts["web"]["port"], json!(2000));
    assert_eq!(opts["web"]["host"], json!("0.0.0.0"));
}

#[test]
fn command_plugin_config_minimal() {
    let toml = "run = \"deny\"\n";
    let partial: PartialCommandPluginConfig = toml::from_str(toml).unwrap();
    assert_eq!(partial.run, Some(RunPolicy::Deny));
    assert!(partial.install.is_none());
    assert!(partial.checksum.is_none());
    assert!(partial.options.is_none());
}
