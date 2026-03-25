use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use jp_tool::{Action, Outcome};
use serde_json::Map;

use super::*;

fn format_ctx() -> Context {
    Context {
        root: Utf8PathBuf::from("/tmp"),
        action: Action::FormatArguments,
    }
}

fn run_ctx(dir: &camino_tempfile::Utf8TempDir) -> Context {
    Context {
        root: dir.path().to_path_buf(),
        action: Action::Run,
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
    let ctx = format_ctx();
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
async fn format_without_content() {
    let ctx = format_ctx();
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
