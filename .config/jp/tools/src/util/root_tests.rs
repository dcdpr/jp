use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::tempdir;
use jp_tool::{AccessPolicy, Capability, FsRule, Outcome};
use pretty_assertions::assert_eq;
use serde_json::{Map, json};

use super::{CARGO_MANIFEST, GIT_DIR, configured_root, note_root, resolve_root};

/// Enough of a capability set for the authorization tests to be able to fail.
const READS: &[Capability] = &[Capability::Read];

/// Create `relative` under `root` as a cargo package.
///
/// A bare directory is not enough: the resolver requires a manifest, because
/// without one cargo would search parent directories and operate on the
/// enclosing workspace.
fn package_dir(root: &Utf8Path, relative: &str) -> Utf8PathBuf {
    let dir = root.join(relative);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"game\"\n").unwrap();
    dir
}

/// Create `relative` under `root` as an ordinary git checkout.
fn repo_dir(root: &Utf8Path, relative: &str) -> Utf8PathBuf {
    let dir = root.join(relative);
    fs::create_dir_all(dir.join(".git")).unwrap();
    dir
}

fn options(value: &serde_json::Value) -> Map<String, serde_json::Value> {
    value.as_object().unwrap().clone()
}

#[test]
fn an_absent_option_leaves_the_tool_on_its_invocation_root() {
    assert_eq!(configured_root(&Map::new()).unwrap(), None);
    assert_eq!(
        configured_root(&options(&json!({"root": null}))).unwrap(),
        None
    );
}

#[test]
fn a_string_option_is_read_as_written() {
    assert_eq!(
        configured_root(&options(&json!({"root": "crates/game"}))).unwrap(),
        Some("crates/game")
    );
}

/// A malformed value reported as an absent one would run the tool against the
/// workspace, and report success, after the user asked for another directory.
#[test]
fn a_non_string_option_is_refused_rather_than_ignored() {
    let error = configured_root(&options(&json!({"root": ["crates/game"]}))).unwrap_err();

    assert!(error.contains("must be a string"), "got: {error}");
}

#[test]
fn no_option_keeps_the_invocation_root() {
    let dir = tempdir().unwrap();

    assert_eq!(
        resolve_root(dir.path(), None, None, READS, &CARGO_MANIFEST)
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
        resolve_root(dir.path(), Some(""), None, READS, &CARGO_MANIFEST)
            .unwrap()
            .as_path(),
        dir.path()
    );
    assert_eq!(
        resolve_root(dir.path(), Some("."), None, READS, &CARGO_MANIFEST)
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
        resolve_root(
            dir.path(),
            Some("crates/game"),
            None,
            READS,
            &CARGO_MANIFEST
        )
        .unwrap(),
        nested.canonicalize_utf8().unwrap()
    );
}

/// The program that runs in the resolved directory is not sandboxed, so the
/// root is confined to the workspace exactly like every other tool path.
/// An approved `external` mount is the only sanctioned way out.
#[test]
fn an_absolute_option_is_rejected() {
    let dir = tempdir().unwrap();
    let outside = tempdir().unwrap();

    let error = resolve_root(
        dir.path(),
        Some(outside.path().as_str()),
        None,
        READS,
        &CARGO_MANIFEST,
    )
    .unwrap_err();

    assert!(error.contains("must be relative"), "got: {error}");
}

#[test]
fn a_parent_traversal_is_rejected() {
    let dir = tempdir().unwrap();

    let error = resolve_root(dir.path(), Some("../.."), None, READS, &CARGO_MANIFEST).unwrap_err();

    assert!(error.contains("escape"), "got: {error}");
}

/// The resolved path embeds a temp directory, so this pins the two facts that
/// matter rather than the whole message.
#[test]
fn a_missing_directory_is_rejected() {
    let dir = tempdir().unwrap();

    let error = resolve_root(
        dir.path(),
        Some("crates/typo"),
        None,
        READS,
        &CARGO_MANIFEST,
    )
    .unwrap_err();

    assert!(error.contains("crates/typo"), "got: {error}");
    assert!(error.contains("not a directory"), "got: {error}");
}

/// Naming a file inside the directory rather than the directory itself is the
/// likely mistake; the program would otherwise fail somewhere far less obvious.
#[test]
fn a_file_is_rejected() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("Cargo.toml"), "").unwrap();

    let error =
        resolve_root(dir.path(), Some("Cargo.toml"), None, READS, &CARGO_MANIFEST).unwrap_err();

    assert!(error.contains("not a directory"), "got: {error}");
}

/// A directory without a manifest is not a cargo workspace: cargo would search
/// parent directories and rewrite the enclosing workspace instead, reporting
/// success while touching a project the caller never named.
#[test]
fn a_directory_without_a_manifest_is_rejected() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("crates")).unwrap();

    let error = resolve_root(dir.path(), Some("crates"), None, READS, &CARGO_MANIFEST).unwrap_err();

    assert!(error.contains("no `Cargo.toml`"), "got: {error}");
    assert!(error.contains("enclosing workspace"), "got: {error}");
}

/// Without the check, a root one directory above the repository would send
/// every write tool — `git_commit` included — at the enclosing repository.
#[test]
fn a_directory_without_a_git_dir_is_rejected() {
    let dir = tempdir().unwrap();
    repo_dir(dir.path(), "vendor/project");

    let error = resolve_root(dir.path(), Some("vendor"), None, READS, &GIT_DIR).unwrap_err();

    assert!(error.contains("no `.git`"), "got: {error}");
    assert!(error.contains("enclosing repository"), "got: {error}");
}

#[test]
fn an_ordinary_checkout_is_accepted() {
    let dir = tempdir().unwrap();
    let repo = repo_dir(dir.path(), "vendor/project");

    assert_eq!(
        resolve_root(dir.path(), Some("vendor/project"), None, READS, &GIT_DIR).unwrap(),
        repo.canonicalize_utf8().unwrap()
    );
}

/// In a worktree or a submodule `.git` is a file pointing at the real git
/// directory, and git works there exactly as it does in a checkout.
#[test]
fn a_worktree_whose_git_is_a_file_is_accepted() {
    let dir = tempdir().unwrap();
    let worktree = dir.path().join("vendor/worktree");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(
        worktree.join(".git"),
        "gitdir: /elsewhere/.git/worktrees/wt\n",
    )
    .unwrap();

    assert_eq!(
        resolve_root(dir.path(), Some("vendor/worktree"), None, READS, &GIT_DIR).unwrap(),
        worktree.canonicalize_utf8().unwrap()
    );
}

/// `external` only lets a path resolve outside the workspace; it grants
/// nothing.
#[test]
fn a_configured_root_needs_the_capabilities_it_was_asked_for() {
    let dir = tempdir().unwrap();
    package_dir(dir.path(), "crates/game");

    let policy = AccessPolicy {
        fs: vec![FsRule::new("crates/game").with_read(true)],
        ..AccessPolicy::default()
    };

    assert!(
        resolve_root(
            dir.path(),
            Some("crates/game"),
            Some(&policy),
            &[Capability::Read],
            &CARGO_MANIFEST,
        )
        .is_ok()
    );

    let error = resolve_root(
        dir.path(),
        Some("crates/game"),
        Some(&policy),
        &[Capability::Read, Capability::Update],
        &CARGO_MANIFEST,
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
        resolve_root(dir.path(), None, Some(&policy), READS, &CARGO_MANIFEST)
            .unwrap()
            .as_path(),
        dir.path()
    );
}

/// Without the directory named, the program's own message gives no hint that it
/// ran somewhere other than the workspace.
#[test]
fn an_error_names_the_redirected_root() {
    let outcome = Ok(Outcome::Error {
        message: "package ID specification `tools` did not match any packages".to_owned(),
        trace: vec![],
        transient: false,
    });

    let annotated = note_root(outcome, Utf8Path::new("crates/my-workspace"), "cargo").unwrap();

    let Outcome::Error { message, .. } = annotated else {
        panic!("expected an error outcome");
    };
    assert!(
        message.contains("did not match any packages"),
        "the original message must survive, got: {message}"
    );
    assert!(message.contains("crates/my-workspace"), "got: {message}");
    assert!(message.contains("cargo ran in"), "got: {message}");
}

/// The caller asked for the redirect, so a success needs no restating.
#[test]
fn a_success_is_left_alone() {
    let outcome = Ok(Outcome::Success {
        content: "Check succeeded.".to_owned(),
    });

    let annotated = note_root(outcome, Utf8Path::new("crates/my-workspace"), "cargo").unwrap();

    assert_eq!(annotated, Outcome::Success {
        content: "Check succeeded.".to_owned()
    });
}
