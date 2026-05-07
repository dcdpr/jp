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
fn unstaged_basic() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["diff-files", "-p", "--", "src/main.rs"])
        .returns_success(small_diff());

    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
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
fn staged_basic() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "diff-index",
            "--cached",
            "--ita-invisible-in-index",
            "-p",
            "HEAD",
            "--",
            "src/main.rs",
        ])
        .returns_success(small_diff());

    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Staged,
        &["src/main.rs"],
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("println"));
}

#[test]
fn truncates_large_output() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(600));

    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
        &["big.rs"],
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("[Showing 500/604 lines."));
}

#[test]
fn pattern_filters_large_output() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(600));

    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
        &["big.rs"],
        Some("line 42:"),
        Some(0),
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("line 42:"));
    assert!(content.contains("[Showing"));
    // Pattern mode should never use the truncation note's `pattern` hint.
    assert!(!content.contains("Use the `pattern` parameter"));
}

#[test]
fn empty_diff() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
        &["nonexistent.rs"],
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(content, "No changes for the specified paths.");
}

#[test]
fn git_error() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::error("fatal: not a git repository");

    let outcome = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
        &["file.rs"],
        None,
        None,
        &runner,
        &[],
    )
    .unwrap();

    assert!(outcome.into_content().is_none(), "expected error outcome");
}
