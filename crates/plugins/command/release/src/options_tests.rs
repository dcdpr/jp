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
    let options = read(json!({ "cadence": "2w" })).unwrap();

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
fn bump_is_split_into_words_and_takes_the_version() {
    let options =
        read(json!({ "bump": "cargo set-version --package 'jp cli' {version}" })).unwrap();

    let (program, args) = options
        .bump
        .unwrap()
        .command(&Version::parse("0.3.0").unwrap());
    assert_eq!(program, "cargo");
    assert_eq!(args, ["set-version", "--package", "jp cli", "0.3.0"]);
}

#[test]
fn every_version_placeholder_takes_the_version() {
    let options = read(json!({ "bump": "sed -i s/.*/{version}/ VERSION-{version}" })).unwrap();

    let (_, args) = options
        .bump
        .unwrap()
        .command(&Version::parse("1.0.0").unwrap());
    assert_eq!(args, ["-i", "s/.*/1.0.0/", "VERSION-1.0.0"]);
}

#[test]
fn a_bump_that_cannot_write_the_version_is_refused() {
    assert_eq!(
        read(json!({ "bump": "cargo set-version" })),
        Err(
            "`plugins.command.release.options.bump` names no `{version}`, so it couldn't write \
             the new version."
                .to_owned()
        )
    );
    assert_eq!(
        read(json!({ "bump": "  " })),
        Err("`plugins.command.release.options.bump` is empty.".to_owned())
    );
    assert_eq!(
        read(json!({ "bump": "cargo set-version '{version}" })),
        Err(
            "`plugins.command.release.options.bump` can't be split into words; check its quotes."
                .to_owned()
        )
    );
}

#[test]
fn change_request_labels_are_trimmed_and_listed_once() {
    let options = read(json!({
        "change_request_labels": ["no-changelog", " release ", "no-changelog"]
    }))
    .unwrap();

    assert_eq!(options.change_request_labels, ["no-changelog", "release"]);
    assert_eq!(
        read(json!({ "change_request_labels": [""] })),
        Err(
            "`plugins.command.release.options.change_request_labels` lists an empty label."
                .to_owned()
        )
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
