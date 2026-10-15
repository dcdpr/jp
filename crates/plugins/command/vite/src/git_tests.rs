use std::fs;

use camino_tempfile::Utf8TempDir;

use super::*;

/// Run git in `dir`, failing the test with its stderr.
fn git(dir: &Utf8Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));

    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A repository with one commit holding two pages in `sub/docs/rfd/`.
///
/// The config is set locally so the machine's identity and signing setup can't
/// reach in and change what the test does.
fn repository() -> Utf8TempDir {
    let top = Utf8TempDir::new().unwrap();
    git(top.path(), &["init", "--quiet", "--initial-branch=main"]);
    git(top.path(), &["config", "user.name", "Tester"]);
    git(top.path(), &["config", "user.email", "tester@example.com"]);
    git(top.path(), &["config", "commit.gpgsign", "false"]);

    fs::create_dir_all(top.path().join("sub/docs/rfd")).unwrap();
    fs::write(top.path().join("sub/docs/rfd/index.md"), "Index").unwrap();
    fs::write(top.path().join("sub/docs/rfd/priority.md"), "Priority").unwrap();
    git(top.path(), &["add", "."]);
    git(top.path(), &["commit", "--quiet", "-m", "Pages"]);

    top
}

#[test]
fn the_prefix_is_the_root_below_the_top_of_the_work_tree() {
    let top = repository();

    assert_eq!(prefix(top.path()), Ok(Some(String::new())));
    assert_eq!(prefix(&top.path().join("sub")), Ok(Some("sub/".to_owned())));
}

#[test]
fn a_directory_outside_a_work_tree_has_no_prefix() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(prefix(dir.path()), Ok(None));
}

/// What git recorded can be restored; a change it hasn't, or a file it doesn't
/// track, can't.
#[test]
fn modified_and_untracked_files_are_unrecorded() {
    let top = repository();
    let root = top.path().join("sub");
    fs::write(root.join("docs/rfd/index.md"), "Edited").unwrap();
    fs::write(root.join("docs/rfd/extra.md"), "New").unwrap();

    let found = unrecorded(&root, "sub/", &[
        Utf8PathBuf::from("docs/rfd/index.md"),
        Utf8PathBuf::from("docs/rfd/priority.md"),
        Utf8PathBuf::from("docs/rfd/extra.md"),
    ])
    .unwrap();

    assert_eq!(found, [
        (
            Utf8PathBuf::from("docs/rfd/extra.md"),
            Unrecorded::Untracked
        ),
        (Utf8PathBuf::from("docs/rfd/index.md"), Unrecorded::Modified),
    ]);
}

#[test]
fn a_staged_change_is_still_unrecorded() {
    let top = repository();
    fs::write(top.path().join("sub/docs/rfd/index.md"), "Staged").unwrap();
    git(top.path(), &["add", "sub/docs/rfd/index.md"]);

    let found = unrecorded(top.path(), "", &[Utf8PathBuf::from(
        "sub/docs/rfd/index.md",
    )])
    .unwrap();

    assert_eq!(found, [(
        Utf8PathBuf::from("sub/docs/rfd/index.md"),
        Unrecorded::Modified
    )]);
}

#[test]
fn the_origin_remote_is_read_as_configured() {
    let top = repository();
    assert_eq!(origin(top.path()), None);

    git(top.path(), &[
        "remote",
        "add",
        "origin",
        "git@github.com:dcdpr/jp.git",
    ]);
    assert_eq!(
        origin(top.path()).as_deref(),
        Some("git@github.com:dcdpr/jp.git")
    );
}

#[test]
fn status_entries_are_read_below_the_prefix() {
    let output = "?? sub/docs/new.md\0 M sub/docs/index.md\0M  other/file.md\0";

    assert_eq!(parse_status(output, "sub/"), [
        (Utf8PathBuf::from("docs/new.md"), Unrecorded::Untracked),
        (Utf8PathBuf::from("docs/index.md"), Unrecorded::Modified),
    ]);
}
