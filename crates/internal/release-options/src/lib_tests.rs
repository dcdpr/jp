use serde_json::json;

use super::*;

#[test]
fn without_settings_the_fragments_are_in_changelog_d() {
    assert_eq!(
        Notes::from_config(&json!({})),
        Ok(Notes {
            fragments_dir: Utf8PathBuf::from("changelog.d"),
            forge: None,
            repository: None,
            headings: Headings::default(),
        })
    );
}

/// Settings only `jp release` reads, such as `bump`, are left to it.
#[test]
fn the_settings_are_read_from_the_whole_configuration() {
    let config = json!({ "plugins": { "command": { "release": { "options": {
        "fragments_dir": "changes",
        "forge": "none",
        "repository": "https://github.com/dcdpr/jp/",
        "headings": { "security": " Security fixes " },
        "bump": "cargo set-version {version}",
    }}}}});
    let mut headings = Headings::default();
    headings.set(Type::Security, "Security fixes");

    assert_eq!(
        Notes::from_config(&config),
        Ok(Notes {
            fragments_dir: Utf8PathBuf::from("changes"),
            forge: Some(Forge::None),
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
            headings,
        })
    );
}

#[test]
fn the_settings_are_read_from_the_options_jp_release_is_passed() {
    let Value::Object(options) = json!({ "forge": "gitlab" }) else {
        unreachable!()
    };

    assert_eq!(
        Notes::read(&options),
        Ok(Notes {
            forge: Some(Forge::GitLab),
            ..Notes::default()
        })
    );
}

#[test]
fn invalid_settings_are_refused_naming_the_setting() {
    let refused = |options: Value| {
        Notes::from_config(&json!({
            "plugins": { "command": { "release": { "options": options } } }
        }))
        .unwrap_err()
    };

    assert_eq!(
        refused(json!({ "fragments_dir": " " })),
        "`plugins.command.release.options.fragments_dir` is empty."
    );
    assert_eq!(
        refused(json!({ "forge": "GitHub" })),
        "`plugins.command.release.options.forge`: `GitHub` is not a forge; use `github`, \
         `gitlab`, or `none`."
    );
    assert!(
        refused(json!({ "headings": { "feat": "Features" } })).starts_with(
            "`plugins.command.release.options.headings`: `feat` is not a changelog type"
        ),
    );
    assert_eq!(
        refused(json!({ "headings": { "fixed": " " } })),
        "`plugins.command.release.options.headings.fixed` is empty."
    );
    assert_eq!(
        refused(json!({ "repository": "git@github.com:dcdpr/jp.git" })),
        "`plugins.command.release.options.repository` must be an `https://` address."
    );
    assert!(
        refused(json!({ "fragments_dir": 3 }))
            .starts_with("`plugins.command.release.options` is invalid: "),
    );
}

#[test]
fn the_address_error_names_the_forge_and_the_setting() {
    assert_eq!(
        no_address(Forge::GitHub),
        "The forge is `github`, but git's `origin` remote has no web address for credits to link \
         to; set `plugins.command.release.options.repository` to the project's, such as \
         `https://github.com/dcdpr/jp`."
    );
}
