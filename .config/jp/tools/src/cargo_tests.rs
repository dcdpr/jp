use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, Action, Capability, Context, FsRule, Outcome};
use pretty_assertions::assert_eq;
use serde_json::{Map, json};

use super::{Tool, cargo_root, note_root, required_capabilities, run};

/// Capabilities for a building subcommand, which is the demanding case.
const BUILDS: &[Capability] = &[
    Capability::Read,
    Capability::Create,
    Capability::Update,
    Capability::Delete,
    Capability::Execute,
];

/// Create `relative` under `root` as a cargo package.
///
/// A bare directory is not enough: `cargo_root` requires a manifest, because
/// without one cargo would search parent directories and operate on the
/// enclosing workspace.
fn package_dir(root: &Utf8Path, relative: &str) -> Utf8PathBuf {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"game\"\n").unwrap();
    dir
}

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
    let nested = package_dir(dir.path(), "crates/game");

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
    package_dir(dir.path(), "crates/game");

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

/// `fmt` rewrites sources in place and does nothing else.
#[test]
fn formatting_needs_no_more_than_read_and_update() {
    assert_eq!(required_capabilities("format"), &[
        Capability::Read,
        Capability::Update
    ]);
}

/// `cargo update` writes a `Cargo.lock` when the target has none, so declaring
/// only read plus update would authorize a command that creates a file the
/// policy denied.
#[test]
fn updating_needs_create_for_a_missing_lockfile() {
    assert_eq!(required_capabilities("update"), &[
        Capability::Read,
        Capability::Create,
        Capability::Update
    ]);
}

/// Compiling writes artifacts, removes stale ones, and runs build scripts, proc
/// macros and test binaries.
/// None of that is sandboxed, so the declaration has to cover deletion too.
#[test]
fn building_subcommands_declare_every_capability() {
    for subcommand in ["check", "test", "expand"] {
        let capabilities = required_capabilities(subcommand);

        for required in [
            Capability::Read,
            Capability::Create,
            Capability::Update,
            Capability::Delete,
            Capability::Execute,
        ] {
            assert!(
                capabilities.contains(&required),
                "{subcommand} can {}",
                required.as_str()
            );
        }
    }
}

/// A directory without a manifest is not a cargo workspace: cargo would search
/// parent directories and rewrite the enclosing workspace instead, reporting
/// success while touching a project the caller never named.
#[test]
fn a_directory_without_a_manifest_is_rejected() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("crates")).unwrap();

    let error = cargo_root(dir.path(), Some("crates"), None, BUILDS).unwrap_err();

    assert!(error.contains("no `Cargo.toml`"), "got: {error}");
    assert!(error.contains("enclosing workspace"), "got: {error}");
}

/// `Tool::option_or` reports a malformed value as an absent one, which would
/// silently run against the host workspace while reporting success.
/// Every other test here calls `cargo_root` directly, so only driving `run`
/// catches a regression to reading the option through `option_or`.
#[tokio::test]
async fn a_non_string_root_option_fails_the_invocation() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };
    let tool = Tool {
        name: "cargo_check".to_owned(),
        arguments: Map::new(),
        answers: Map::new(),
        options: Map::from_iter([("root".to_owned(), json!(1))]),
    };

    // Returns before cargo is spawned, so no process fixture is needed.
    let Outcome::Error { message, .. } = run(ctx, tool).await.unwrap() else {
        panic!("expected an error outcome");
    };

    assert!(message.contains("must be a string"), "got: {message}");
}
