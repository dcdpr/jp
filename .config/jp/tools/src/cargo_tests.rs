use std::{fs, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, Action, Capability, Context, FsRule, Outcome};
use pretty_assertions::assert_eq;
use serde_json::{Map, json};

use super::{
    Tool, cargo_root, format_duration, note_duration, note_root, required_capabilities, run,
    rustflags,
};

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
