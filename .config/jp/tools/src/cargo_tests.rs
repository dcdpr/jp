use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, Action, Capability, Context, FsRule, Outcome};
use pretty_assertions::assert_eq;
use serde_json::{Map, json};

use super::{Tool, required_capabilities, run};
use crate::util::root::{CARGO_MANIFEST, resolve_root};

/// Create `relative` under `root` as a cargo package.
///
/// A bare directory is not enough: the root resolver requires a manifest,
/// because without one cargo would search parent directories and operate on the
/// enclosing workspace.
fn package_dir(root: &Utf8Path, relative: &str) -> Utf8PathBuf {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"game\"\n").unwrap();
    dir
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
        resolve_root(
            dir.path(),
            Some("crates/game"),
            Some(&policy),
            required_capabilities("format"),
            &CARGO_MANIFEST,
        )
        .is_ok()
    );

    // `cargo check` additionally writes artifacts and executes build scripts.
    let error = resolve_root(
        dir.path(),
        Some("crates/game"),
        Some(&policy),
        required_capabilities("check"),
        &CARGO_MANIFEST,
    )
    .unwrap_err();

    assert!(error.contains("Access denied"), "got: {error}");
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

/// `Tool::option_or` reports a malformed value as an absent one, which would
/// silently run against the host workspace while reporting success.
/// The resolver itself is covered in `util/root_tests.rs`, so only driving
/// `run` catches a regression to reading the option through `option_or`.
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
