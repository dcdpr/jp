use changelog::{Forge, Type};
use serde_json::json;

use super::*;

fn options(value: Value) -> Map<String, Value> {
    let Value::Object(options) = value else {
        panic!("options are a table");
    };

    options
}

#[test]
fn without_options_nothing_extra_is_attached_or_added() {
    assert_eq!(Options::read(&Map::new()), Ok(Options::default()));
}

#[test]
fn jps_options_are_read() {
    let read = Options::read(&options(json!({
        "assets": [".config/release/install.sh", ".config/release/install.ps1"],
        "upgrade_guide": "https://jp.computer/releases/{version}/upgrade",
        "footer": ".config/release/github.md",
    })));

    assert_eq!(
        read,
        Ok(Options {
            assets: vec![
                Utf8PathBuf::from(".config/release/install.sh"),
                Utf8PathBuf::from(".config/release/install.ps1"),
            ],
            upgrade_guide: Some("https://jp.computer/releases/{version}/upgrade".to_owned()),
            footer: Some(Utf8PathBuf::from(".config/release/github.md")),
        })
    );
}

#[test]
fn settings_the_plugin_does_not_read_are_ignored() {
    assert_eq!(
        Options::read(&options(json!({ "cadence": "2w" }))),
        Ok(Options::default())
    );
}

/// Every release's guide is at its own address, so one address for all of them
/// would link breaking entries to the wrong release.
#[test]
fn an_upgrade_guide_has_to_name_the_version() {
    let read = Options::read(&options(json!({
        "upgrade_guide": "https://jp.computer/releases/latest/upgrade",
    })));

    assert_eq!(
        read,
        Err(
            "`plugins.command.release-github.options.upgrade_guide` has to name the release's \
             version as `{version}`, such as `https://example.com/releases/{version}/upgrade`."
                .to_owned()
        )
    );
}

#[test]
fn an_upgrade_guide_has_to_be_a_web_address() {
    let read = Options::read(&options(json!({
        "upgrade_guide": "releases/{version}/upgrade",
    })));

    assert_eq!(
        read,
        Err(
            "`plugins.command.release-github.options.upgrade_guide` must be an `https://` address."
                .to_owned()
        )
    );
}

#[test]
fn an_empty_asset_is_refused() {
    let read = Options::read(&options(
        json!({ "assets": [".config/release/install.sh", " "] }),
    ));

    assert_eq!(
        read,
        Err("`plugins.command.release-github.options.assets` lists an empty path.".to_owned())
    );
}

#[test]
fn notes_read_jp_releases_settings() {
    let config = json!({ "plugins": { "command": { "release": { "options": {
        "fragments_dir": "notes",
        "forge": "github",
        "repository": "https://github.com/dcdpr/jp/",
        "headings": { "added": "New features" },
        "bump": "cargo set-version {version}",
    }}}}});
    let mut headings = Headings::default();
    headings.set(Type::Added, "New features");

    assert_eq!(
        Notes::read(&config),
        Ok(Notes {
            dir: Utf8PathBuf::from("notes"),
            forge: Some(Forge::GitHub),
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
            headings,
        })
    );
}

#[test]
fn notes_without_jp_releases_settings_read_changelog_d() {
    assert_eq!(
        Notes::read(&json!({})),
        Ok(Notes {
            dir: Utf8PathBuf::from("changelog.d"),
            ..Notes::default()
        })
    );
}

#[test]
fn notes_refuse_a_repository_that_is_no_web_address() {
    let config = json!({ "plugins": { "command": { "release": { "options": {
        "repository": "git@github.com:dcdpr/jp.git",
    }}}}});

    assert_eq!(
        Notes::read(&config),
        Err(
            "`plugins.command.release.options.repository` must be an `https://` address."
                .to_owned()
        )
    );
}

#[test]
fn notes_refuse_a_heading_for_a_type_there_is_none_of() {
    let config = json!({ "plugins": { "command": { "release": { "options": {
        "headings": { "bugs": "Bug fixes" },
    }}}}});

    let error = Notes::read(&config).unwrap_err();

    assert!(
        error.starts_with("`plugins.command.release.options.headings`: "),
        "{error}"
    );
}
