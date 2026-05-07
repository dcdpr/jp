use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

fn small_diff() -> &'static str {
    "\
diff --git a/src/main.rs b/src/main.rs
index abc..def 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"world\");
 }"
}

fn large_diff(line_count: usize) -> String {
    let mut lines = vec![
        "diff --git a/big.rs b/big.rs".to_string(),
        "--- a/big.rs".to_string(),
        "+++ b/big.rs".to_string(),
        "@@ -1,1000 +1,1000 @@".to_string(),
    ];

    for i in 0..line_count {
        lines.push(format!("+line {i}: some generated content here"));
    }

    lines.join("\n")
}

#[test]
fn basic_diff_commit() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(small_diff());

    let content = git_diff_commit_impl(
        dir.path(),
        "abc123",
        &["src/main.rs"],
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.starts_with("```diff\n"));
    assert!(content.ends_with("\n```"));
    assert!(content.contains("println"));
}

#[test]
fn diff_commit_with_pattern() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(small_diff());

    let content = git_diff_commit_impl(
        dir.path(),
        "abc123",
        &["src/main.rs"],
        Some("world"),
        Some(1),
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("```diff\n"));
    assert!(content.contains("world"));
    // The closing diff fence is followed by the `[Showing ...]` note,
    // so we check that both the fence and the note are present rather
    // than that the output ends with the fence.
    assert!(content.contains("\n```\n"), "got: {content}");
    assert!(content.contains("[Showing"), "got: {content}");
}

#[test]
fn diff_commit_empty_diff() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_diff_commit_impl(
        dir.path(),
        "abc123",
        &["nonexistent.rs"],
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(
        content,
        "No diff found for the specified revision and paths."
    );
}

#[test]
fn diff_commit_truncates_large_output() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(600));

    let content = git_diff_commit_impl(dir.path(), "abc123", &["big.rs"], None, None, &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert!(content.contains("[Showing 500/604 lines."));
}

#[test]
fn diff_commit_git_error() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::error("fatal: bad revision");

    let outcome =
        git_diff_commit_impl(dir.path(), "bad", &["file.rs"], None, None, &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none(), "expected error outcome");
}
