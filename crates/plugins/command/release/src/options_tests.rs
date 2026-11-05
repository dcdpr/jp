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
    let options = read(json!({ "cadence": "2w", "forge": "github" })).unwrap();

    assert_eq!(options, Options::default());
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
