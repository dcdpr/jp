use camino_tempfile::tempdir;
use jp_tool::{Action, Outcome};
use serde_json::Map;

use super::*;

fn format_ctx(dir: &camino_tempfile::Utf8TempDir) -> Context {
    Context {
        root: dir.path().to_path_buf(),
        action: Action::FormatArguments,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    }
}

fn run_ctx(dir: &camino_tempfile::Utf8TempDir) -> Context {
    Context {
        root: dir.path().to_path_buf(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    }
}

fn unwrap_content(outcome: Outcome) -> String {
    match outcome {
        Outcome::Success { content } => content,
        other => panic!("expected Success, got {other:?}"),
    }
}

#[tokio::test]
async fn format_with_content_contains_ansi() {
    let dir = tempdir().unwrap();
    let ctx = format_ctx(&dir);
    let answers = Map::new();
    let content = Some("fn main() {}\n".to_owned());

    let result = fs_create_file(ctx, &answers, "src/main.rs".to_owned(), content)
        .await
        .unwrap();
    let output = unwrap_content(result);

    // The output should contain ANSI escape codes from syntax highlighting.
    assert!(
        output.contains("\x1b["),
        "expected ANSI escape codes in output, got:\n{output}"
    );
    // Should not contain raw markdown fences — those should have been consumed
    // by the formatter.
    assert!(
        !output.contains("`````"),
        "expected no raw markdown fences in output, got:\n{output}"
    );
    // The code content should still be present (unhighlighted text).
    assert!(output.contains("fn"), "expected code content in output");
    assert!(output.contains("main"), "expected code content in output");
}

#[tokio::test]
async fn format_rejects_absolute_path() {
    let dir = tempdir().unwrap();
    let ctx = format_ctx(&dir);
    let answers = Map::new();

    let result = fs_create_file(
        ctx,
        &answers,
        "/tmp/repro.rs".to_owned(),
        Some("fn main() {}\n".to_owned()),
    )
    .await
    .unwrap();

    match result {
        Outcome::Error { message, .. } => {
            assert!(
                message.contains("Path must be relative"),
                "unexpected error message: {message}"
            );
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
}

#[tokio::test]
async fn format_without_content() {
    let dir = tempdir().unwrap();
    let ctx = format_ctx(&dir);
    let answers = Map::new();

    let result = fs_create_file(ctx, &answers, "src/empty.rs".to_owned(), None)
        .await
        .unwrap();
    let output = unwrap_content(result);

    assert!(output.contains("empty.rs"), "should mention the file path");
    assert!(
        !output.contains("`````"),
        "no code fence when there's no content"
    );
}

#[tokio::test]
async fn run_creates_file() {
    let dir = tempdir().unwrap();
    let ctx = run_ctx(&dir);
    let answers = Map::new();

    let result = fs_create_file(
        ctx,
        &answers,
        "hello.txt".to_owned(),
        Some("hello world".to_owned()),
    )
    .await
    .unwrap();

    let output = unwrap_content(result);
    assert!(output.contains("hello.txt"), "should mention file path");

    let written = std::fs::read_to_string(dir.path().join("hello.txt")).unwrap();
    assert_eq!(written, "hello world");
}

#[cfg(unix)]
#[tokio::test]
async fn run_refuses_to_write_through_dangling_symlink() {
    // End-to-end regression: a dangling symlink in the workspace pointing
    // outside it must not let `fs_create_file` create the target file.
    // The resolver rejects the path before any I/O happens.
    let outside = tempdir().unwrap();
    let escape_target = outside.path().join("escape.txt");

    let workspace = tempdir().unwrap();
    std::os::unix::fs::symlink(
        escape_target.as_std_path(),
        workspace.path().join("link").as_std_path(),
    )
    .unwrap();

    let ctx = run_ctx(&workspace);
    let answers = Map::new();

    let result = fs_create_file(ctx, &answers, "link".to_owned(), Some("pwned".to_owned()))
        .await
        .unwrap();

    match result {
        Outcome::Error { .. } => {}
        other => panic!("expected Error outcome, got {other:?}"),
    }
    assert!(
        !escape_target.exists(),
        "workspace-escape file was created at {escape_target}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn run_refuses_to_write_through_live_symlink() {
    // Even if the symlink target is inside the workspace, refuse to write
    // through it — the user clearly meant to operate on whichever entry
    // they named. They can delete the link and try again.
    let workspace = tempdir().unwrap();
    std::fs::write(workspace.path().join("real.txt"), "original").unwrap();
    std::os::unix::fs::symlink(
        std::path::Path::new("real.txt"),
        workspace.path().join("link.txt").as_std_path(),
    )
    .unwrap();

    let ctx = run_ctx(&workspace);
    let answers = Map::new();

    let result = fs_create_file(
        ctx,
        &answers,
        "link.txt".to_owned(),
        Some("replacement".to_owned()),
    )
    .await
    .unwrap();

    match result {
        Outcome::Error { message, .. } => {
            assert!(
                message.contains("symlink"),
                "unexpected error message: {message}"
            );
        }
        other => panic!("expected Error outcome, got {other:?}"),
    }
    // The link target was not modified.
    let content = std::fs::read_to_string(workspace.path().join("real.txt")).unwrap();
    assert_eq!(content, "original");
}
