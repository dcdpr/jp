use std::fs;

use camino_tempfile::tempdir;
use pretty_assertions::assert_eq;

use super::cargo_root;

#[test]
fn no_option_keeps_the_invocation_root() {
    let dir = tempdir().unwrap();

    assert_eq!(cargo_root(dir.path(), None).unwrap().as_path(), dir.path());
}

/// A config layer that unloads a previously-set root can say so without knowing
/// what the default was.
#[test]
fn an_empty_or_dot_option_resolves_to_the_invocation_root() {
    let dir = tempdir().unwrap();

    assert_eq!(
        cargo_root(dir.path(), Some("")).unwrap().as_path(),
        dir.path()
    );
    assert_eq!(
        cargo_root(dir.path(), Some(".")).unwrap().as_path(),
        dir.path()
    );
}

#[test]
fn a_relative_option_is_joined_onto_the_invocation_root() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("crates/game");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        cargo_root(dir.path(), Some("crates/game"))
            .unwrap()
            .as_path(),
        nested.as_path()
    );
}

/// An absolute path deliberately escapes the invocation root, so a checkout
/// living outside the workspace can be targeted on purpose.
#[test]
fn an_absolute_option_is_used_as_is() {
    let dir = tempdir().unwrap();
    let sibling = tempdir().unwrap();

    assert_eq!(
        cargo_root(dir.path(), Some(sibling.path().as_str()))
            .unwrap()
            .as_path(),
        sibling.path()
    );
}

/// The resolved path embeds a temp directory, so this pins the two facts that
/// matter rather than the whole message.
#[test]
fn a_missing_directory_is_rejected() {
    let dir = tempdir().unwrap();

    let error = cargo_root(dir.path(), Some("crates/typo")).unwrap_err();

    assert!(error.contains("crates/typo"), "got: {error}");
    assert!(error.contains("not a directory"), "got: {error}");
}

/// Naming the manifest instead of the directory holding it is the likely
/// mistake; cargo would otherwise fail somewhere far less obvious.
#[test]
fn a_file_is_rejected() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let error = cargo_root(dir.path(), Some("Cargo.toml")).unwrap_err();

    assert!(error.contains("not a directory"), "got: {error}");
}
