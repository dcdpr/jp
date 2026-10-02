use std::fs;

#[cfg(unix)]
use camino_tempfile::Utf8TempDir;
use camino_tempfile::tempdir;
use jp_config::{
    plugins::command::PartialCommandPluginConfig,
    providers::mcp::{AlgorithmConfig, PartialChecksumConfig},
};
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

fn pinned(value: &str) -> PartialAppConfig {
    let mut config = PartialAppConfig::empty();
    config
        .plugins
        .command
        .insert("titles".to_owned(), PartialCommandPluginConfig {
            checksum: Some(PartialChecksumConfig {
                value: Some(value.to_owned()),
                ..PartialChecksumConfig::default()
            }),
            ..PartialCommandPluginConfig::default()
        });
    config
}

#[test]
fn a_pin_the_binary_matches_lets_it_be_approved() {
    assert_eq!(
        check_pin(
            &pinned("aaa"),
            "titles",
            Utf8Path::new("/bin/jp-titles"),
            "aaa"
        )
        .map_err(|e| e.message.unwrap_or_default()),
        Ok(())
    );
    assert_eq!(
        check_pin(
            &PartialAppConfig::empty(),
            "titles",
            Utf8Path::new("/bin/jp-titles"),
            "aaa"
        )
        .map_err(|e| e.message.unwrap_or_default()),
        Ok(()),
        "nothing pinned"
    );
}

/// A pin in SHA-1 is compared with the binary's SHA-1, not its SHA-256.
#[test]
fn a_sha1_pin_is_compared_with_the_binarys_sha1() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-titles");
    fs::write(&path, "hello").unwrap();
    let sha256 = registry::sha256_file(&path).unwrap();

    let mut config = pinned("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    config
        .plugins
        .command
        .get_mut("titles")
        .and_then(|c| c.checksum.as_mut())
        .unwrap()
        .algorithm = Some(AlgorithmConfig::Sha1);

    assert_eq!(
        check_pin(&config, "titles", &path, &sha256).map_err(|e| e.message.unwrap_or_default()),
        Ok(())
    );
}

#[test]
fn a_pin_the_binary_does_not_match_refuses_it() {
    assert_eq!(
        check_pin(
            &pinned("bbb"),
            "titles",
            Utf8Path::new("/bin/jp-titles"),
            "aaa"
        )
        .map_err(|e| e.message.unwrap_or_default()),
        Err(
            "plugin `titles` binary checksum mismatch.\nexpected: bbb\nactual:   aaa\nThe binary \
             at /bin/jp-titles has changed since it was pinned. Update \
             plugins.command.titles.checksum.value in your config to accept the new binary."
                .to_owned()
        )
    );
}

/// A plugin that answers `describe` as `titles`, and writes `marker` whenever
/// it runs.
#[cfg(unix)]
fn titles_script(path: &Utf8Path, marker: &Utf8Path) {
    use std::os::unix::fs::PermissionsExt as _;

    fs::write(
        path,
        format!(
            r#"#!/bin/sh
# jp-plugin/v1 {{"protocol":1,"description":"Titles","command":["titles"]}}
: > {marker}
read -r msg
echo '{{"type":"describe","protocol":1,"name":"titles","version":"0.1.0","description":"Titles","command":["titles"],"help":"Usage: jp titles"}}'
"#
        ),
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A binary installed under a versioned file name, and linked into `bin` as
/// `jp-titles`.
///
/// Returns the link, and the marker the binary writes when it runs.
#[cfg(unix)]
fn versioned_install(tmp: &Utf8TempDir) -> (Utf8PathBuf, Utf8PathBuf) {
    let opt = tmp.path().join("opt");
    let bin = tmp.path().join("bin");
    fs::create_dir_all(&opt).unwrap();
    fs::create_dir_all(&bin).unwrap();

    let marker = tmp.path().join("ran");
    let target = opt.join("jp-titles-1.2");
    titles_script(&target, &marker);

    let link = bin.join("jp-titles");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    (link, marker)
}

/// Run `jp plugin approve` on `path`, with `cfg` and a user-global
/// configuration and data directory of their own.
#[cfg(unix)]
fn approve(tmp: &Utf8TempDir, path: &Utf8Path, cfg: &[&str]) -> Result<(), String> {
    let _global = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", tmp.path().join("global").as_str());
    let _data = EnvVarGuard::set("JP_USER_DATA_DIR", tmp.path().join("data").as_str());

    let cfg: Vec<KeyValueOrPath> = cfg.iter().map(|arg| arg.parse().unwrap()).collect();
    let (printer, _out, _err) = Printer::memory(jp_printer::OutputFormat::Text);

    Approve {
        path: path.to_owned(),
    }
    .run(&printer, &cfg, &CancellationToken::new())
    .map_err(|e| e.message.unwrap_or_default())
}

/// A deny for the name the link carries holds, though the file it points at is
/// named otherwise; the plugin is never run.
#[cfg(unix)]
#[test]
#[serial(env_vars)]
fn a_link_is_the_plugin_its_name_says() {
    let tmp = tempdir().unwrap();
    let (link, marker) = versioned_install(&tmp);

    assert_eq!(
        approve(&tmp, &link, &["plugins.command.titles.run=deny"]),
        Err(denied_message())
    );
    assert!(!marker.exists(), "the denied plugin was never run");
}

/// An approval given through a link is recorded under the link's name, which is
/// the name dispatch looks it up by.
#[cfg(unix)]
#[test]
#[serial(env_vars)]
fn an_approval_through_a_link_is_recorded_under_the_link_name() {
    let tmp = tempdir().unwrap();
    let (link, marker) = versioned_install(&tmp);

    assert_eq!(approve(&tmp, &link, &[]), Ok(()));
    assert!(marker.exists(), "approving ran the plugin to describe it");

    let approvals = ApprovalStore::load_from(Some(tmp.path().join("data/plugin-approvals.json")));
    assert_eq!(
        approvals.get("titles").map(|a| a.path.clone()),
        Some(link),
        "recorded under `titles`, at the path it was named by"
    );
    assert!(approvals.get("titles-1.2").is_none());
}

/// A binary whose contents do not match its pinned checksum is refused before
/// it runs.
#[cfg(unix)]
#[test]
#[serial(env_vars)]
fn a_binary_that_does_not_match_its_pin_is_never_run() {
    let tmp = tempdir().unwrap();
    let (link, marker) = versioned_install(&tmp);

    let refusal = approve(&tmp, &link, &["plugins.command.titles.checksum.value=bbb"]);

    // The refusal's full wording is pinned by
    // `a_pin_the_binary_does_not_match_refuses_it`; here it names the actual
    // hash of a script whose text holds a temporary path.
    assert!(
        refusal.as_ref().is_err_and(
            |e| e.starts_with("plugin `titles` binary checksum mismatch.\nexpected: bbb\n")
        ),
        "{refusal:?}"
    );
    assert!(!marker.exists(), "the refused plugin was never run");
}
