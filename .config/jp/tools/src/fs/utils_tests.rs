use camino::Utf8PathBuf;
use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn test_is_file_dirty_modified() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    // Second column 'M' indicates modified
    let runner = MockProcessRunner::success(" M test.rs\n");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    assert!(result);
}

#[test]
fn test_is_file_dirty_not_modified() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    // No output means no changes
    let runner = MockProcessRunner::success("");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    assert!(!result);
}

#[test]
fn test_is_file_dirty_not_a_git_repo() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    let runner = MockProcessRunner::error("fatal: not a git repository");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    // Should return false when not in a git repo
    assert!(!result);
}
