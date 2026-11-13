use camino_tempfile::Utf8TempDir;
use serde_json::{Value, json};

use super::*;
use crate::repository_testing::Fixture;

#[test]
fn the_fragments_directory_is_read_below_the_workspace_root() {
    let root = Utf8TempDir::new().unwrap();
    let elsewhere = Utf8TempDir::new().unwrap();
    let outside = elsewhere.path().join("changelog.d");

    assert_eq!(
        below(root.path(), Utf8Path::new("changelog.d")),
        Ok("changelog.d".to_owned())
    );
    assert_eq!(
        below(root.path(), Utf8Path::new("docs/changes")),
        Ok("docs/changes".to_owned())
    );
    assert_eq!(
        below(root.path(), &root.path().join("changelog.d")),
        Ok("changelog.d".to_owned())
    );
    assert_eq!(
        below(root.path(), &outside),
        Err(format!(
            "The fragments directory {outside} is outside the workspace, so no release has it."
        ))
    );
}

/// The fixture's `origin` is a path on disk, on no forge; a configured address
/// names one, unless the forge is configured too.
#[test]
fn the_forge_follows_the_configured_repository() {
    let fixture = Fixture::new();
    let shell = fixture.shell();
    let forge = |options: Value| {
        let Value::Object(options) = options else {
            panic!("options are an object");
        };
        let options = Options::read(&options).unwrap();

        Pipeline::new(&shell, fixture.root(), &options)
            .unwrap()
            .forge()
    };

    assert_eq!(forge(json!({})), Forge::None);
    assert_eq!(
        forge(json!({ "repository": "https://gitlab.com/group/jp" })),
        Forge::GitLab
    );
    assert_eq!(
        forge(json!({ "repository": "https://gitlab.com/group/jp", "forge": "github" })),
        Forge::GitHub
    );
}

#[test]
fn counts_name_what_they_count() {
    assert_eq!(counted(1, "fragment", "fragments"), "1 fragment");
    assert_eq!(counted(3, "fragment", "fragments"), "3 fragments");
}

#[test]
fn the_files_a_release_files_are_counted_by_kind() {
    assert_eq!(files(3, 1), "3 fragments and 1 highlight");
    assert_eq!(files(1, 0), "1 fragment");
    assert_eq!(files(0, 2), "2 highlights");
}

#[test]
fn a_short_hash_is_its_first_seven_characters() {
    assert_eq!(short("4e1f0a2c9d8b7a6f5e4d"), "4e1f0a2");
    assert_eq!(short("4e1f"), "4e1f");
}
