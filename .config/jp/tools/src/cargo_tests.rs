use std::fs;

use camino::Utf8Path;
use camino_tempfile::tempdir;
use jp_tool::Outcome;
use pretty_assertions::assert_eq;

use super::{cargo_root, note_root};

#[test]
fn no_option_keeps_the_invocation_root() {
    let dir = tempdir().unwrap();

    assert_eq!(
        cargo_root(dir.path(), None, None).unwrap().as_path(),
        dir.path()
    );
}

/// A config layer that unloads a previously-set root can say so without knowing
/// what the default was.
#[test]
fn an_empty_or_dot_option_resolves_to_the_invocation_root() {
    let dir = tempdir().unwrap();

    assert_eq!(
        cargo_root(dir.path(), Some(""), None).unwrap().as_path(),
        dir.path()
    );
    assert_eq!(
        cargo_root(dir.path(), Some("."), None).unwrap().as_path(),
        dir.path()
    );
}

/// The resolver canonicalizes, so the expectation has to be canonical too: on
/// macOS a temp dir under `/var` resolves to `/private/var`.
#[test]
fn a_relative_option_is_joined_onto_the_invocation_root() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("crates/game");
    fs::create_dir_all(&nested).unwrap();

    assert_eq!(
        cargo_root(dir.path(), Some("crates/game"), None).unwrap(),
        nested.canonicalize_utf8().unwrap()
    );
}

/// `cargo` runs build scripts and proc macros, so the root is confined to the
/// workspace exactly like every other tool path.
/// An approved `external` mount is the only sanctioned way out.
#[test]
fn an_absolute_option_is_rejected() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();

    let error = cargo_root(dir.path(), Some(outside.path().as_str()), None).unwrap_err();

    assert!(error.contains("must be relative"), "got: {error}");
}

#[test]
fn a_parent_traversal_is_rejected() {
    let dir = tempdir().unwrap();

    let error = cargo_root(dir.path(), Some("../.."), None).unwrap_err();

    assert!(error.contains("escape"), "got: {error}");
}

/// The resolved path embeds a temp directory, so this pins the two facts that
/// matter rather than the whole message.
#[test]
fn a_missing_directory_is_rejected() {
    let dir = tempdir().unwrap();

    let error = cargo_root(dir.path(), Some("crates/typo"), None).unwrap_err();

    assert!(error.contains("crates/typo"), "got: {error}");
    assert!(error.contains("not a directory"), "got: {error}");
}

/// Without the directory named, cargo's own message gives no hint that it ran
/// somewhere other than the workspace.
#[test]
fn an_error_names_the_redirected_root() {
    let outcome = Ok(Outcome::Error {
        message: "package ID specification `tools` did not match any packages".to_owned(),
        trace: vec![],
        transient: false,
    });

    let annotated = note_root(outcome, Utf8Path::new("crates/my-workspace")).unwrap();

    let Outcome::Error { message, .. } = annotated else {
        panic!("expected an error outcome");
    };
    assert!(
        message.contains("did not match any packages"),
        "the original message must survive, got: {message}"
    );
    assert!(message.contains("crates/my-workspace"), "got: {message}");
}

/// The caller asked for the redirect, so a success needs no restating.
#[test]
fn a_success_is_left_alone() {
    let outcome = Ok(Outcome::Success {
        content: "Check succeeded.".to_owned(),
    });

    let annotated = note_root(outcome, Utf8Path::new("crates/my-workspace")).unwrap();

    assert_eq!(annotated, Outcome::Success {
        content: "Check succeeded.".to_owned()
    });
}

/// Naming the manifest instead of the directory holding it is the likely
/// mistake; cargo would otherwise fail somewhere far less obvious.
#[test]
fn a_file_is_rejected() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let error = cargo_root(dir.path(), Some("Cargo.toml"), None).unwrap_err();

    assert!(error.contains("not a directory"), "got: {error}");
}
