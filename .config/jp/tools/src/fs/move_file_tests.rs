use camino_tempfile::tempdir;
use jp_tool::Outcome;
use serde_json::{Map, Value, json};

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput};

fn no_answers() -> Map<String, Value> {
    Map::new()
}

fn answers(pairs: &[(&str, Value)]) -> Map<String, Value> {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert((*k).to_owned(), v.clone());
    }
    m
}

fn clean_git_runner() -> MockProcessRunner {
    // `git status --porcelain -- <path>` returns empty stdout, success.
    MockProcessRunner::builder()
        .expect("git")
        .returns_success("")
}

fn dirty_git_runner(porcelain: &str) -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect("git")
        .returns(ProcessOutput {
            stdout: porcelain.to_owned(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
}

fn never_git_runner() -> MockProcessRunner {
    MockProcessRunner::never_called()
}

fn unwrap_success(o: Outcome) -> String {
    match o {
        Outcome::Success { content } => content,
        other => panic!("expected Success, got {other:?}"),
    }
}

fn unwrap_error(o: Outcome) -> String {
    match o {
        Outcome::Error { message, .. } => message,
        other => panic!("expected Error, got {other:?}"),
    }
}

fn unwrap_needs_input(o: Outcome) -> jp_tool::Question {
    match o {
        Outcome::NeedsInput { question } => question,
        other => panic!("expected NeedsInput, got {other:?}"),
    }
}

#[test]
fn moves_file_to_new_path() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "hello").unwrap();

    let result =
        fs_move_file_impl(root, &no_answers(), "a.txt", "b.txt", &clean_git_runner()).unwrap();

    let msg = unwrap_success(result);
    assert!(msg.contains("Moved file"), "unexpected message: {msg}");
    assert!(!root.join("a.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("b.txt")).unwrap(),
        "hello"
    );
}

#[test]
fn moves_directory_with_contents() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("old/nested")).unwrap();
    std::fs::write(root.join("old/a.txt"), "1").unwrap();
    std::fs::write(root.join("old/nested/b.txt"), "2").unwrap();

    let result = fs_move_file_impl(root, &no_answers(), "old", "new", &clean_git_runner()).unwrap();

    let msg = unwrap_success(result);
    assert!(msg.contains("Moved directory"), "unexpected message: {msg}");
    assert!(!root.join("old").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("new/a.txt")).unwrap(),
        "1"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("new/nested/b.txt")).unwrap(),
        "2"
    );
}

#[test]
fn moves_directory_creates_target_parents() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/foo.rs"), "").unwrap();

    let result = fs_move_file_impl(
        root,
        &no_answers(),
        "src",
        "vendored/upstream/src",
        &clean_git_runner(),
    )
    .unwrap();

    unwrap_success(result);
    assert!(root.join("vendored/upstream/src/foo.rs").exists());
}

#[test]
fn missing_source_errors() {
    let dir = tempdir().unwrap();
    let result = fs_move_file_impl(
        dir.path(),
        &no_answers(),
        "ghost.txt",
        "elsewhere.txt",
        &never_git_runner(),
    )
    .unwrap();

    let msg = unwrap_error(result);
    assert!(msg.contains("does not exist"), "unexpected: {msg}");
}

#[test]
fn workspace_escape_rejected() {
    let dir = tempdir().unwrap();
    let result = fs_move_file_impl(
        dir.path(),
        &no_answers(),
        "../escape.txt",
        "inside.txt",
        &never_git_runner(),
    )
    .unwrap();

    let msg = unwrap_error(result);
    assert!(msg.contains("escape the workspace"), "unexpected: {msg}");
}

#[test]
fn file_target_is_directory_errors() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    std::fs::create_dir(root.join("target_dir")).unwrap();

    let result = fs_move_file_impl(
        root,
        &no_answers(),
        "a.txt",
        "target_dir",
        &never_git_runner(),
    )
    .unwrap();

    let msg = unwrap_error(result);
    assert!(msg.contains("existing directory"), "unexpected: {msg}");
}

#[test]
fn file_target_exists_prompts_for_overwrite() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    std::fs::write(root.join("b.txt"), "y").unwrap();

    let result =
        fs_move_file_impl(root, &no_answers(), "a.txt", "b.txt", &never_git_runner()).unwrap();

    let question = unwrap_needs_input(result);
    assert_eq!(question.id, "overwrite_file");
    assert!(question.text.contains("Overwrite"));
}

#[test]
fn file_overwrite_approved_succeeds() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    std::fs::write(root.join("b.txt"), "y").unwrap();

    let answers = answers(&[("overwrite_file", json!(true))]);
    let result = fs_move_file_impl(root, &answers, "a.txt", "b.txt", &clean_git_runner()).unwrap();

    unwrap_success(result);
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "x");
}

#[test]
fn file_overwrite_denied_errors() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();
    std::fs::write(root.join("b.txt"), "y").unwrap();

    let answers = answers(&[("overwrite_file", json!(false))]);
    let result = fs_move_file_impl(root, &answers, "a.txt", "b.txt", &never_git_runner()).unwrap();

    let msg = unwrap_error(result);
    assert!(msg.contains("already exists"), "unexpected: {msg}");
}

#[test]
fn dir_target_exists_errors_without_prompt() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "").unwrap();
    std::fs::create_dir(root.join("dst")).unwrap();

    let result = fs_move_file_impl(root, &no_answers(), "src", "dst", &never_git_runner()).unwrap();

    let msg = unwrap_error(result);
    assert!(msg.contains("already exists"), "unexpected: {msg}");
    // The directory should still exist intact.
    assert!(root.join("src/a.rs").exists());
}

#[test]
fn dir_target_exists_as_file_errors() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("dst"), "").unwrap();

    let result = fs_move_file_impl(root, &no_answers(), "src", "dst", &never_git_runner()).unwrap();

    unwrap_error(result);
}

#[test]
fn dirty_file_prompts_for_confirmation() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();

    let runner = dirty_git_runner(" M a.txt\n");
    let result = fs_move_file_impl(root, &no_answers(), "a.txt", "b.txt", &runner).unwrap();

    let question = unwrap_needs_input(result);
    assert_eq!(question.id, "move_dirty_source");
    assert!(question.text.contains("File 'a.txt'"));
    assert!(question.text.contains("uncommitted"));
}

#[test]
fn dirty_file_approved_proceeds() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();

    let runner = dirty_git_runner(" M a.txt\n");
    let answers = answers(&[("move_dirty_source", json!(true))]);
    let result = fs_move_file_impl(root, &answers, "a.txt", "b.txt", &runner).unwrap();

    unwrap_success(result);
    assert!(!root.join("a.txt").exists());
    assert!(root.join("b.txt").exists());
}

#[test]
fn dirty_directory_prompts_with_entry_count() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("d")).unwrap();
    std::fs::write(root.join("d/a"), "").unwrap();
    std::fs::write(root.join("d/b"), "").unwrap();
    std::fs::write(root.join("d/c"), "").unwrap();

    let runner = dirty_git_runner(" M d/a\n M d/b\n?? d/c\n");
    let result = fs_move_file_impl(root, &no_answers(), "d", "renamed", &runner).unwrap();

    let question = unwrap_needs_input(result);
    assert_eq!(question.id, "move_dirty_source");
    assert!(question.text.contains("Directory 'd'"));
    assert!(question.text.contains("3 uncommitted entries"));
}

#[test]
fn clean_directory_skips_dirty_prompt() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("d")).unwrap();
    std::fs::write(root.join("d/a"), "").unwrap();

    let result =
        fs_move_file_impl(root, &no_answers(), "d", "renamed", &clean_git_runner()).unwrap();

    unwrap_success(result);
    assert!(root.join("renamed/a").exists());
}

#[test]
fn source_equals_target_errors() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("a.txt"), "x").unwrap();

    let result =
        fs_move_file_impl(root, &no_answers(), "a.txt", "a.txt", &never_git_runner()).unwrap();

    let msg = unwrap_error(result);
    assert!(msg.contains("same path"), "unexpected error: {msg}");
}

#[test]
fn empty_parent_directory_is_removed() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("nested/dir")).unwrap();
    std::fs::write(root.join("nested/dir/file.txt"), "").unwrap();

    let result = fs_move_file_impl(
        root,
        &no_answers(),
        "nested/dir/file.txt",
        "file.txt",
        &clean_git_runner(),
    )
    .unwrap();

    let msg = unwrap_success(result);
    assert!(
        msg.contains("Removed empty parent"),
        "expected parent-cleanup note in: {msg}"
    );
    assert!(!root.join("nested/dir").exists());
}

#[cfg(unix)]
#[test]
fn symlink_source_moves_target_file() {
    // Documented surprise: `resolve_workspace_path` canonicalizes the source
    // through the symlink, so we end up moving the target file rather than
    // renaming the link. The link itself becomes dangling. See the
    // `SourceKind` doc comment for the rationale.
    let dir = tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("real.txt"), "payload").unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new("real.txt"),
        root.join("link.txt").as_std_path(),
    )
    .unwrap();

    let result = fs_move_file_impl(
        root,
        &no_answers(),
        "link.txt",
        "moved.txt",
        &clean_git_runner(),
    )
    .unwrap();

    unwrap_success(result);
    // The underlying file moved; the symlink is left dangling.
    assert!(!root.join("real.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("moved.txt")).unwrap(),
        "payload"
    );
    let link_meta = std::fs::symlink_metadata(root.join("link.txt")).unwrap();
    assert!(link_meta.file_type().is_symlink());
    assert!(!root.join("link.txt").exists()); // exists() follows the dangling link
}
