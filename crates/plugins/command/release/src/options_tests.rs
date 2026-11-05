use serde_json::json;

use super::*;

fn read(options: Value) -> Result<Options, String> {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    Options::read(&options)
}

#[test]
fn the_fragments_directory_defaults_to_changelog_d() {
    assert_eq!(read(json!({})).unwrap().fragments_dir, "changelog.d");
}

#[test]
fn the_fragments_directory_can_be_named() {
    let options = read(json!({ "fragments_dir": "docs/changes" })).unwrap();

    assert_eq!(options.fragments_dir, "docs/changes");
}

/// A configuration written for a newer plugin, with settings this one doesn't
/// know, still loads.
#[test]
fn settings_this_plugin_does_not_read_are_ignored() {
    let options = read(json!({ "cadence": "2w", "bump": "cargo set-version {version}" })).unwrap();

    assert_eq!(options, Options::default());
}

/// Unset, the forge is read from git's `origin` remote when a release is read.
#[test]
fn the_forge_is_unset_by_default() {
    assert_eq!(read(json!({})).unwrap().forge, None);
}

#[test]
fn the_forge_is_named_as_github_gitlab_or_none() {
    assert_eq!(
        read(json!({ "forge": "gitlab" })).unwrap().forge,
        Some(Forge::GitLab)
    );
    assert_eq!(
        read(json!({ "forge": "none" })).unwrap().forge,
        Some(Forge::None)
    );

    assert_eq!(
        read(json!({ "forge": "GitHub" })),
        Err(
            "`plugins.command.release.options.forge`: `GitHub` is not a forge; use `github`, \
             `gitlab`, or `none`."
                .to_owned()
        )
    );
}

#[test]
fn headings_rename_the_types_they_name() {
    let options = read(json!({
        "headings": { "added": "New features", "fixed": " Bug fixes " }
    }))
    .unwrap();

    assert_eq!(options.headings.of(Type::Added), "New features");
    assert_eq!(options.headings.of(Type::Fixed), "Bug fixes");
    assert_eq!(options.headings.of(Type::Changed), "Changed");
}

#[test]
fn a_heading_for_something_that_is_not_a_type_is_refused() {
    assert_eq!(
        read(json!({ "headings": { "feat": "Features" } })),
        Err(
            "`plugins.command.release.options.headings`: `feat` is not a changelog type; use one \
             of: added, changed, deprecated, removed, fixed, security, performance."
                .to_owned()
        )
    );
}

#[test]
fn an_empty_heading_is_refused() {
    assert_eq!(
        read(json!({ "headings": { "fixed": " " } })),
        Err("`plugins.command.release.options.headings.fixed` is empty.".to_owned())
    );
}

#[test]
fn an_empty_fragments_directory_is_refused() {
    assert_eq!(
        read(json!({ "fragments_dir": " " })),
        Err("`plugins.command.release.options.fragments_dir` is empty.".to_owned())
    );
}

#[test]
fn a_fragments_directory_that_is_not_a_path_is_refused() {
    let error = read(json!({ "fragments_dir": 3 })).unwrap_err();

    assert!(
        error.starts_with("`plugins.command.release.options` is invalid: "),
        "{error}"
    );
}
