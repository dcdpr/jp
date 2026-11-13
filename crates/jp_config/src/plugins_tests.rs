use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::PartialAppConfig;

/// The shutdown timeout is a command plugin setting, so a file that still sets
/// it for every kind of plugin is refused.
#[test]
fn a_shutdown_timeout_outside_the_command_section_is_refused_in_a_file() {
    let error =
        serde_json::from_value::<PartialPluginsConfig>(json!({ "shutdown_timeout_secs": 20 }))
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "unknown field `shutdown_timeout_secs`, expected `command`"
    );
}

/// `--cfg` names the key the setting moved to, where a file's parser only says
/// which keys it knows.
#[test]
fn a_shutdown_timeout_outside_the_command_section_is_refused_naming_where_it_went() {
    let mut config = PartialAppConfig::empty();

    let error = config
        .assign(KvAssignment::try_from_cli("plugins.shutdown_timeout_secs", "20").unwrap())
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "plugins.shutdown_timeout_secs: moved to `plugins.command.*.shutdown_timeout_secs`"
    );
    assert_eq!(config, PartialAppConfig::empty());
}

/// `JP_CFG_PLUGINS_SHUTDOWN_TIMEOUT_SECS`, as the environment layer reads it.
#[test]
fn a_shutdown_timeout_outside_the_command_section_is_refused_from_the_environment() {
    let mut config = PartialAppConfig::empty();

    let error = config
        .assign(KvAssignment::try_from_env("plugins_shutdown_timeout_secs", "20").unwrap())
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "plugins_shutdown_timeout_secs: moved to `plugins.command.*.shutdown_timeout_secs`"
    );
}
