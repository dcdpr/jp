use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn test_git_unstage_single_file() {
    let dir = tempdir().unwrap();

    let runner = MockProcessRunner::success("");

    let result = git_unstage_impl(dir.path(), &["test.rs".to_string()], &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert_eq!(result, "Changes unstaged.");
}

#[test]
fn test_git_unstage_multiple_files() {
    let dir = tempdir().unwrap();

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["restore", "--staged", "--", "file1.rs"])
        .returns_success("")
        .expect("git")
        .args(&["restore", "--staged", "--", "file2.rs"])
        .returns_success("");

    let result = git_unstage_impl(
        dir.path(),
        &["file1.rs".to_string(), "file2.rs".to_string()],
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(result, "Changes unstaged.");
}
