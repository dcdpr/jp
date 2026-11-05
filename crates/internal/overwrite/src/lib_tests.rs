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
fn the_prefix_is_the_dir_below_the_top_of_the_work_tree() {
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
    let dir = top.path().join("sub");
    fs::write(dir.join("docs/rfd/index.md"), "Edited").unwrap();
    fs::write(dir.join("docs/rfd/extra.md"), "New").unwrap();

    let found = unrecorded(&dir, &[
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

/// Paths are taken against the directory they are asked about, wherever it sits
/// in the work tree.
#[test]
fn paths_are_relative_to_the_directory() {
    let top = repository();
    let dir = top.path().join("sub/docs/rfd");
    fs::write(dir.join("index.md"), "Edited").unwrap();

    let found = unrecorded(&dir, &[
        Utf8PathBuf::from("index.md"),
        Utf8PathBuf::from("priority.md"),
    ])
    .unwrap();

    assert_eq!(found, [(
        Utf8PathBuf::from("index.md"),
        Unrecorded::Modified
    )]);
}

#[test]
fn a_staged_change_is_still_unrecorded() {
    let top = repository();
    fs::write(top.path().join("sub/docs/rfd/index.md"), "Staged").unwrap();
    git(top.path(), &["add", "sub/docs/rfd/index.md"]);

    let found = unrecorded(top.path(), &[Utf8PathBuf::from("sub/docs/rfd/index.md")]).unwrap();

    assert_eq!(found, [(
        Utf8PathBuf::from("sub/docs/rfd/index.md"),
        Unrecorded::Modified
    )]);
}

/// `git status` leaves ignored files out unless asked, and git has no more copy
/// of an ignored file than of any other untracked one.
#[test]
fn ignored_files_are_untracked() {
    let top = repository();
    let dir = top.path().join("sub");
    fs::write(
        top.path().join(".gitignore"),
        "sub/docs/generated/\nsub/docs/rfd/notes.md\n",
    )
    .unwrap();
    git(top.path(), &["add", ".gitignore"]);
    git(top.path(), &["commit", "--quiet", "-m", "Ignore"]);
    fs::create_dir_all(dir.join("docs/generated")).unwrap();
    fs::write(dir.join("docs/generated/config.mjs"), "Edited by hand").unwrap();
    fs::write(dir.join("docs/rfd/notes.md"), "Notes").unwrap();

    let found = unrecorded(&dir, &[
        Utf8PathBuf::from("docs/generated/config.mjs"),
        Utf8PathBuf::from("docs/rfd/notes.md"),
        Utf8PathBuf::from("docs/rfd/index.md"),
    ])
    .unwrap();

    assert_eq!(found, [
        (
            Utf8PathBuf::from("docs/generated/config.mjs"),
            Unrecorded::Untracked
        ),
        (
            Utf8PathBuf::from("docs/rfd/notes.md"),
            Unrecorded::Untracked
        ),
    ]);
}

/// Outside a work tree nothing can restore a file, however it got there.
#[test]
fn every_file_outside_a_work_tree_is_unrecorded() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("b.toml"), "B").unwrap();
    fs::write(dir.path().join("a.toml"), "A").unwrap();

    let found = unrecorded(dir.path(), &[
        Utf8PathBuf::from("b.toml"),
        Utf8PathBuf::from("a.toml"),
    ])
    .unwrap();

    assert_eq!(found, [
        (Utf8PathBuf::from("a.toml"), Unrecorded::NoWorkTree),
        (Utf8PathBuf::from("b.toml"), Unrecorded::NoWorkTree),
    ]);
}

/// Nothing to overwrite asks git nothing, so a directory that doesn't exist yet
/// is fine.
#[test]
fn no_paths_are_never_unrecorded() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(unrecorded(&dir.path().join("missing"), &[]), Ok(vec![]));
}

#[test]
fn status_entries_are_read_below_the_prefix() {
    let output =
        "?? sub/docs/new.md\0 M sub/docs/index.md\0!! sub/docs/ignored.md\0M  other/file.md\0";

    assert_eq!(parse_status(output, "sub/"), [
        (Utf8PathBuf::from("docs/new.md"), Unrecorded::Untracked),
        (Utf8PathBuf::from("docs/index.md"), Unrecorded::Modified),
        (Utf8PathBuf::from("docs/ignored.md"), Unrecorded::Untracked),
    ]);
}

#[test]
fn a_refusal_in_a_work_tree_suggests_committing() {
    let message = refusal("jp vite sync", &[
        (Utf8PathBuf::from("a.mjs"), Unrecorded::Modified),
        (Utf8PathBuf::from("b.mjs"), Unrecorded::Untracked),
    ]);

    assert_eq!(
        message,
        "Git couldn't restore these files, so `jp vite sync` leaves them:\n  a.mjs (changed since \
         the last commit)\n  b.mjs (not tracked)\nCommit them, or run `jp vite sync --force` to \
         overwrite them anyway."
    );
}

/// There is nothing to commit to outside a work tree, so only `--force` is
/// offered.
#[test]
fn a_refusal_outside_a_work_tree_offers_only_force() {
    let message = refusal("jp rfd init", &[(
        Utf8PathBuf::from("docs/rfd/000-design-template.md"),
        Unrecorded::NoWorkTree,
    )]);

    assert_eq!(
        message,
        "Git couldn't restore these files, so `jp rfd init` leaves them:\n  \
         docs/rfd/000-design-template.md (not in a git work tree)\nRun `jp rfd init --force` to \
         overwrite them anyway."
    );
}

#[test]
fn existing_compares_a_file_with_what_would_be_written() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join("a.toml");

    assert_eq!(existing(&path, b"a\n"), Ok(Existing::Missing));

    fs::write(&path, "a\r\n").unwrap();
    assert_eq!(existing(&path, b"a\n"), Ok(Existing::Same));
    assert_eq!(existing(&path, b"b\n"), Ok(Existing::Differs));
}

#[test]
fn only_a_carriage_return_ending_a_line_is_ignored() {
    assert!(same_text(b"a\r\nb\r\n", b"a\nb\n"));
    assert!(same_text(b"a\nb", b"a\r\nb"));
    assert!(!same_text(b"a\rb\n", b"ab\n"));
    assert!(!same_text(b"a\r\n", b"a\n\n"));
}
