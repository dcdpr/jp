use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, Action, Capability, Context, FsRule, Outcome};
use pretty_assertions::assert_eq;
use serde_json::{Map, json};

use super::{Tool, ensure_workspace_root, required_capabilities, run};
use crate::util::{
    root::{CARGO_MANIFEST, resolve_root},
    runner::MockProcessRunner,
};

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

/// Build a runner that answers one `cargo locate-project` for `root` with
/// `workspace` as the manifest it resolves to.
fn locate_project(root: &Utf8Path, workspace: &Utf8Path) -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "locate-project",
            "--workspace",
            "--message-format=plain",
            "--manifest-path",
            root.join("Cargo.toml").as_str(),
        ])
        .returns_success(format!("{}\n", workspace.join("Cargo.toml")))
}

/// A member has a manifest, so the marker check clears it, but cargo resolves
/// the workspace from that manifest: `--workspace`, `--all` and the lockfile
/// would all land on the enclosing project instead.
#[test]
fn a_workspace_member_is_refused() {
    let dir = tempdir().unwrap();
    let member = package_dir(dir.path(), "crates/game");
    let workspace = dir.path().canonicalize_utf8().unwrap();

    let runner = locate_project(&member, &workspace);

    let error = ensure_workspace_root(&member, &runner).unwrap_err();

    assert!(error.contains("crates/game"), "got: {error}");
    assert!(error.contains(workspace.as_str()), "got: {error}");
    assert!(
        error.contains("member of the cargo workspace"),
        "got: {error}"
    );
}

#[test]
fn a_workspace_root_is_accepted() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize_utf8().unwrap();
    fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();

    let runner = locate_project(&root, &root);

    ensure_workspace_root(&root, &runner).unwrap();
}

/// Falling through on a manifest cargo cannot read would hand the question back
/// to the build command, which answers it by walking up.
#[test]
fn a_workspace_cargo_cannot_resolve_is_refused() {
    let dir = tempdir().unwrap();
    let root = dir.path().canonicalize_utf8().unwrap();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_error("error: failed to parse manifest");

    let error = ensure_workspace_root(&root, &runner).unwrap_err();

    assert!(error.contains("failed to parse manifest"), "got: {error}");
}

/// `cargo_install_tools` builds through `just install-tools`, whose recipe pins
/// its own profile, so the option cannot be honored there.
/// Ignoring it would put the build back in the shared target directory —
/// taking the lock and rewriting the fingerprints a concurrent build relies on
/// — and report success.
#[tokio::test]
async fn a_profile_on_a_subcommand_that_cannot_use_it_fails_the_invocation() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };
    let tool = Tool {
        name: "cargo_install_tools".to_owned(),
        arguments: Map::new(),
        answers: Map::new(),
        options: Map::from_iter([("profile".to_owned(), json!("agent"))]),
    };

    // Returns before `just` is spawned, so no process fixture is needed.
    let Outcome::Error { message, .. } = run(ctx, tool).await.unwrap() else {
        panic!("expected an error outcome");
    };

    assert!(
        message.contains("no effect on `cargo_install_tools`"),
        "got: {message}"
    );
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
