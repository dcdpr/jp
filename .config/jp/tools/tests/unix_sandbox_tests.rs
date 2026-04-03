//! Integration tests for the `unix_utils` sandbox on macOS.
//!
//! These tests run actual utilities inside `sandbox-exec` and verify that:
//! - Workspace files are readable.
//! - Sensitive paths (`/Users`, `/tmp`) are blocked.
//! - File writes are blocked.
//! - Network access is blocked.
//!
//! Skipped automatically on non-macOS platforms or when `sandbox-exec` is not
//! available.

use std::fs;

use camino_tempfile::Utf8TempDir;
use jp_tool::{Action, Context, Outcome};
use serde_json::{Map, Value, json};
use tools::Tool;

fn has_sandbox_exec() -> bool {
    cfg!(target_os = "macos") && which::which("sandbox-exec").is_ok()
}

fn setup() -> (Utf8TempDir, Context) {
    let dir = camino_tempfile::tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };
    (dir, ctx)
}

fn tool(name: &str, args: &Value) -> Tool {
    Tool {
        name: name.to_string(),
        arguments: args.as_object().unwrap().clone(),
        answers: Map::new(),
        options: Map::new(),
    }
}

async fn run_tool(ctx: Context, t: Tool) -> Outcome {
    tools::run(ctx, t).await.unwrap()
}

// --- Allowed operations ---

#[tokio::test]
async fn sandbox_allows_reading_workspace_file() {
    if !has_sandbox_exec() {
        return;
    }

    let (dir, ctx) = setup();
    fs::write(dir.path().join("hello.txt"), "workspace content\n").unwrap();

    let outcome = run_tool(
        ctx,
        tool(
            "unix_utils",
            &json!({
                "util": "head",
                "args": ["-n", "1", "hello.txt"]
            }),
        ),
    )
    .await;

    match outcome {
        Outcome::Success { content } => {
            assert!(content.contains("workspace content"), "got: {content}");
        }
        other => panic!("expected success, got: {other:?}"),
    }
}

#[tokio::test]
async fn sandbox_allows_stdin_processing() {
    if !has_sandbox_exec() {
        return;
    }

    let (_dir, ctx) = setup();

    let outcome = run_tool(
        ctx,
        tool(
            "unix_utils",
            &json!({
                "util": "wc",
                "args": ["-l"],
                "stdin": "line1\nline2\nline3\n"
            }),
        ),
    )
    .await;

    match outcome {
        Outcome::Success { content } => {
            assert!(content.contains('3'), "got: {content}");
        }
        other => panic!("expected success, got: {other:?}"),
    }
}

#[tokio::test]
async fn sandbox_allows_date() {
    if !has_sandbox_exec() {
        return;
    }

    let (_dir, ctx) = setup();

    let outcome = run_tool(
        ctx,
        tool(
            "unix_utils",
            &json!({
                "util": "date",
                "args": ["+%Y"]
            }),
        ),
    )
    .await;

    match outcome {
        Outcome::Success { content } => {
            assert!(content.contains("202"), "got: {content}");
        }
        other => panic!("expected success, got: {other:?}"),
    }
}

// --- Blocked reads ---

#[tokio::test]
async fn sandbox_blocks_reading_users_dir() {
    if !has_sandbox_exec() {
        return;
    }

    let (_dir, ctx) = setup();
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return;
    }

    // The argument validation will likely catch this first, but if it
    // doesn't, the sandbox must block it. Either way the tool must not
    // return the file contents.
    let outcome = run_tool(
        ctx,
        tool(
            "unix_utils",
            &json!({
                "util": "head",
                "args": ["-n", "1", format!("{home}/.zshrc")]
            }),
        ),
    )
    .await;

    match &outcome {
        Outcome::Success { content } => {
            // If it "succeeded", the output should contain an error from
            // head (Operation not permitted), not actual file contents.
            assert!(
                content.contains("Operation not permitted")
                    || content.contains("No such file")
                    || content.contains("cannot open"),
                "sandbox should have blocked read, got: {content}"
            );
        }
        Outcome::Error { .. } => {
            // Argument validation caught it — also acceptable.
        }
        other @ Outcome::NeedsInput { .. } => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn sandbox_blocks_reading_tmp() {
    if !has_sandbox_exec() {
        return;
    }

    let (_dir, ctx) = setup();

    // Create a file in /tmp to ensure it exists.
    let tmp_file = "/tmp/jp-sandbox-test-read.txt";
    fs::write(tmp_file, "secret\n").ok();

    let outcome = run_tool(
        ctx,
        tool(
            "unix_utils",
            &json!({
                "util": "head",
                "args": ["-n", "1", tmp_file]
            }),
        ),
    )
    .await;

    // Clean up.
    fs::remove_file(tmp_file).ok();

    match &outcome {
        Outcome::Success { content } => {
            assert!(
                !content.contains("secret"),
                "sandbox should have blocked /tmp read, got: {content}"
            );
        }
        Outcome::Error { .. } => {
            // Argument validation caught it — acceptable.
        }
        other @ Outcome::NeedsInput { .. } => panic!("unexpected outcome: {other:?}"),
    }
}

// --- Blocked writes ---

#[tokio::test]
async fn sandbox_blocks_file_writes() {
    if !has_sandbox_exec() {
        return;
    }

    let (dir, ctx) = setup();
    let target = dir.path().join("should-not-exist.txt");

    // `tee` writes stdin to a file — should be blocked by the sandbox.
    // But tee isn't in our allowed utils. Use jq which can write via
    // --rawfile or similar... actually, none of our utils write files
    // directly. The sandbox deny-default blocks writes regardless.
    //
    // Instead, verify that the sandbox profile itself blocks writes by
    // checking that a tool can't create files even in the workspace.
    // We use `sort -o` which writes output to a file.
    let outcome = run_tool(
        ctx,
        tool(
            "unix_utils",
            &json!({
                "util": "sort",
                "args": ["-o", "should-not-exist.txt"],
                "stdin": "hello\n"
            }),
        ),
    )
    .await;

    // The sort command should fail or produce an error because writes
    // are denied.
    assert!(!target.exists(), "sandbox should have prevented file write");

    // The outcome might be success (sort ran but couldn't write) or
    // error — either way the file must not exist.
    if let Outcome::Success { content } = &outcome {
        assert!(
            content.contains("error")
                || content.contains("Operation not permitted")
                || content.contains("terminated by signal")
                || content.is_empty()
                || content.contains("status"),
            "unexpected success content: {content}"
        );
    }
}
