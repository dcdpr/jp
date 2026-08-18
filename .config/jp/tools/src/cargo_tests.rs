use std::fs;

use camino::Utf8Path;
use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, Capability, FsRule, Outcome};
use pretty_assertions::assert_eq;

use super::{cargo_root, note_root, required_capabilities};

/// Capabilities for a building subcommand, which is the demanding case.
const BUILDS: &[Capability] = &[
    Capability::Read,
    Capability::Create,
    Capability::Update,
    Capability::Execute,
];

#[test]
fn no_option_keeps_the_invocation_root() {
    let dir = tempdir().unwrap();

    assert_eq!(
        cargo_root(dir.path(), None, None, BUILDS)
            .unwrap()
            .as_path(),
        dir.path()
    );
}

/// A config layer that unloads a previously-set root can say so without knowing
/// what the default was.
#[test]
fn an_empty_or_dot_option_resolves_to_the_invocation_root() {
    let dir = tempdir().unwrap();

    assert_eq!(
        cargo_root(dir.path(), Some(""), None, BUILDS)
            .unwrap()
            .as_path(),
        dir.path()
    );
    assert_eq!(
        cargo_root(dir.path(), Some("."), None, BUILDS)
            .unwrap()
            .as_path(),
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
        cargo_root(dir.path(), Some("crates/game"), None, BUILDS).unwrap(),
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

    let error = cargo_root(dir.path(), Some(outside.path().as_str()), None, BUILDS).unwrap_err();

    assert!(error.contains("must be relative"), "got: {error}");
}

#[test]
fn a_parent_traversal_is_rejected() {
    let dir = tempdir().unwrap();

    let error = cargo_root(dir.path(), Some("../.."), None, BUILDS).unwrap_err();

    assert!(error.contains("escape"), "got: {error}");
}

/// The resolved path embeds a temp directory, so this pins the two facts that
/// matter rather than the whole message.
#[test]
fn a_missing_directory_is_rejected() {
    let dir = tempdir().unwrap();

    let error = cargo_root(dir.path(), Some("crates/typo"), None, BUILDS).unwrap_err();

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

    let error = cargo_root(dir.path(), Some("Cargo.toml"), None, BUILDS).unwrap_err();

    assert!(error.contains("not a directory"), "got: {error}");
}

/// `external` only lets a path resolve outside the workspace; it grants
/// nothing.
/// A mount the user opened for reading and formatting must not become a licence
/// to run build scripts and proc macros there.
#[test]
fn a_configured_root_needs_the_capabilities_its_subcommand_uses() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("crates/game")).unwrap();

    let policy = AccessPolicy {
        fs: vec![FsRule::new("crates/game").with_read(true).with_update(true)],
        ..AccessPolicy::default()
    };

    // `cargo fmt` reads and rewrites sources, both of which are granted.
    assert!(
        cargo_root(
            dir.path(),
            Some("crates/game"),
            Some(&policy),
            required_capabilities("format"),
        )
        .is_ok()
    );

    // `cargo check` additionally writes artifacts and executes build scripts.
    let error = cargo_root(
        dir.path(),
        Some("crates/game"),
        Some(&policy),
        required_capabilities("check"),
    )
    .unwrap_err();

    assert!(error.contains("Access denied"), "got: {error}");
}

/// The invocation root predates this option, so a restrictive policy must not
/// revoke access the `root` option never handed out.
#[test]
fn the_invocation_root_is_not_authorized() {
    let dir = tempdir().unwrap();

    let policy = AccessPolicy {
        fs: vec![FsRule::new("nowhere").with_read(true)],
        ..AccessPolicy::default()
    };

    assert_eq!(
        cargo_root(dir.path(), None, Some(&policy), BUILDS)
            .unwrap()
            .as_path(),
        dir.path()
    );
}

/// Neither subcommand compiles, so neither needs to create artifacts or execute
/// anything.
#[test]
fn only_building_subcommands_require_create_and_execute() {
    for subcommand in ["format", "update"] {
        let capabilities = required_capabilities(subcommand);
        assert_eq!(
            capabilities,
            &[Capability::Read, Capability::Update],
            "{subcommand} does not compile"
        );
    }

    for subcommand in ["check", "test", "expand"] {
        let capabilities = required_capabilities(subcommand);
        assert!(
            capabilities.contains(&Capability::Execute),
            "{subcommand} runs build scripts and proc macros"
        );
        assert!(
            capabilities.contains(&Capability::Create),
            "{subcommand} writes build artifacts"
        );
    }
}
