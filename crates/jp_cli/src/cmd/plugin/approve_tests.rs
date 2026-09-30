use std::fs;

use camino_tempfile::tempdir;
use pretty_assertions::assert_eq;
use serial_test::serial;

use super::*;
use crate::{CfgKeyword, env_testing::EnvVarGuard};

fn denied_message() -> String {
    "plugin `titles` is denied by configuration (plugins.command.titles.run = \"deny\"), so it is \
     not run to approve it"
        .to_owned()
}

fn check(cfg: &[KeyValueOrPath]) -> Result<(), String> {
    let config = load_user_global_partial(cfg).unwrap();
    check_not_denied(&config, "titles").map_err(|e| e.message.unwrap_or_default())
}

/// `jp --cfg plugins.command.titles.run=deny plugin approve …` refuses, the
/// same way the setting in the user-global config file does.
#[test]
#[serial(env_vars)]
fn a_cfg_argument_can_deny_approving() {
    let tmp = tempdir().unwrap();
    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", tmp.path().as_str());

    assert_eq!(check(&[]), Ok(()));

    let deny = "plugins.command.titles.run=deny"
        .parse::<KeyValueOrPath>()
        .unwrap();
    assert_eq!(check(&[deny]), Err(denied_message()));
}

#[test]
#[serial(env_vars)]
fn the_user_global_config_denies_and_no_cfg_resets_it() {
    let tmp = tempdir().unwrap();
    fs::write(
        tmp.path().join("config.toml"),
        "[plugins.command.titles]\nrun = \"deny\"\n",
    )
    .unwrap();
    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", tmp.path().as_str());

    assert_eq!(check(&[]), Err(denied_message()));
    assert_eq!(
        check(&[KeyValueOrPath::Keyword(CfgKeyword::None)]),
        Ok(()),
        "`--no-cfg` resets the configuration the deny came from"
    );
}
