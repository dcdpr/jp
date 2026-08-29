use std::{fs, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, Action, Capability, Context, FsRule, Outcome};
use pretty_assertions::assert_eq;
use serde_json::{Map, json};

use super::{
    Tool, ensure_workspace_root, format_duration, note_duration, required_capabilities, run,
    rustflags,
};
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

#[test]
fn no_configured_flags_is_just_the_base() {
    assert_eq!(rustflags(&[]), "-W warnings");
}

/// Configured flags come last so they can override the base.
#[test]
fn configured_flags_are_appended_to_the_base() {
    let flags = rustflags(&[
        "-Zthreads=0".to_owned(),
        "-Clink-arg=-fuse-ld=lld".to_owned(),
    ]);

    assert_eq!(flags, "-W warnings -Zthreads=0 -Clink-arg=-fuse-ld=lld");
}

/// An array and a bare string are both accepted, so a single flag needs no
/// brackets.
#[test]
fn a_bare_string_is_accepted_as_one_flag() {
    assert_eq!(
        rustflags(&["-Zthreads=0".to_owned()]),
        "-W warnings -Zthreads=0"
    );
}

/// A malformed value is refused rather than dropped, for the same reason as
/// `root`: compiling with flags the caller believes are in effect is worse than
/// refusing to compile.
#[tokio::test]
async fn a_non_string_rustflags_option_fails_the_invocation() {
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
        options: Map::from_iter([("rustflags".to_owned(), json!({ "flag": true }))]),
    };

    let Outcome::Error { message, .. } = run(ctx, tool).await.unwrap() else {
        panic!("expected an error outcome");
    };

    assert!(
        message.contains("must be a string or an array of strings"),
        "got: {message}"
    );
}

/// A tenth of a second is what separates a warm cache from a small rebuild, so
/// sub-minute durations keep the fraction.
#[test]
fn short_durations_keep_a_fraction() {
    assert_eq!(format_duration(Duration::from_millis(1_240)), "1.2s");
    assert_eq!(format_duration(Duration::from_millis(230)), "0.2s");
    assert_eq!(format_duration(Duration::from_secs(59)), "59.0s");
}

#[test]
fn long_durations_are_minutes_and_seconds() {
    assert_eq!(format_duration(Duration::from_mins(1)), "1m 0s");
    assert_eq!(format_duration(Duration::from_secs(230)), "3m 50s");
}

/// "Check succeeded" reads the same after a warm cache and a full rebuild; the
/// duration is the only thing that tells them apart.
#[test]
fn a_success_carries_its_duration() {
    let outcome = Ok(Outcome::Success {
        content: "Check succeeded. No warnings or errors found.".to_owned(),
    });

    let noted = note_duration(outcome, Duration::from_secs(230)).unwrap();

    assert_eq!(noted, Outcome::Success {
        content: "Check succeeded. No warnings or errors found.\n\n(took 3m 50s)".to_owned(),
    });
}

/// A failure that took three minutes and one that took three seconds call for
/// different responses.
#[test]
fn a_failure_carries_its_duration() {
    let outcome = Ok(Outcome::Error {
        message: "error: could not compile `bevy`".to_owned(),
        trace: vec![],
        transient: false,
    });

    let Outcome::Error { message, .. } = note_duration(outcome, Duration::from_secs(95)).unwrap()
    else {
        panic!("expected an error outcome");
    };

    assert!(message.contains("could not compile"), "got: {message}");
    assert!(message.contains("(took 1m 35s)"), "got: {message}");
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
