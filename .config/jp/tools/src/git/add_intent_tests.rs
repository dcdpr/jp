use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn test_add_intent_single_file() {
    let dir = tempdir().unwrap();

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["add", "--intent-to-add", "--", "new_file.rs"])
        .returns_success("");

    let content = git_add_intent_impl(dir.path(), &["new_file.rs".to_string()], &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert_eq!(
        content,
        "Marked 1 file as intent-to-add. They are now visible to `git_list_patches`."
    );
}

#[test]
fn test_add_intent_multiple_files() {
    let dir = tempdir().unwrap();

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["add", "--intent-to-add", "--", "a.rs"])
        .returns_success("")
        .expect("git")
        .args(&["add", "--intent-to-add", "--", "b.rs"])
        .returns_success("");

    let content = git_add_intent_impl(
        dir.path(),
        &["a.rs".to_string(), "b.rs".to_string()],
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(
        content,
        "Marked 2 files as intent-to-add. They are now visible to `git_list_patches`."
    );
}

#[test]
fn test_add_intent_failure() {
    let dir = tempdir().unwrap();

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["add", "--intent-to-add", "--", "missing.rs"])
        .returns_error("fatal: pathspec 'missing.rs' did not match any files");

    let err =
        git_add_intent_impl(dir.path(), &["missing.rs".to_string()], &runner, &[]).unwrap_err();

    assert!(err.to_string().contains("Failed to intent-to-add"));
    assert!(err.to_string().contains("missing.rs"));
}
