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
fn option_like_revision_is_passed_as_positional() {
    // `--end-of-options` must precede `<rev>` so an option-shaped value
    // reaches git as a positional rather than as an option to `git show`.
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "show",
            "--format=",
            "--end-of-options",
            "--output=/tmp/leak",
            "--",
            "src/main.rs",
        ])
        .returns_success(small_diff());

    let _outcome = git_diff_commit_impl(
        dir.path(),
        "--output=/tmp/leak",
        &["src/main.rs"],
        None,
        None,
        None,
        None,
        &runner,
        &[],
    )
    .unwrap();
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
        None,
        None,
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

    let content = git_diff_commit_impl(
        dir.path(),
        "abc123",
        &["big.rs"],
        None,
        None,
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("[Showing 500/604 lines."));
    assert!(content.contains("`pattern`"));
    assert!(content.contains("`start_line`"));
}

#[test]
fn diff_commit_range_bypasses_cap() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(600));

    let content = git_diff_commit_impl(
        dir.path(),
        "abc123",
        &["big.rs"],
        None,
        None,
        Some(400),
        Some(600),
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("... (starting from line #400) ..."));
    assert!(content.contains("... (truncated after line #600) ..."));
    assert!(!content.contains("[Showing"));
    // Lines past the 500-line cap must be present.
    assert!(content.contains("line 500:"));
    assert!(content.contains("line 595:"));
}

#[test]
fn diff_commit_range_with_pattern() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(600));

    let content = git_diff_commit_impl(
        dir.path(),
        "abc123",
        &["big.rs"],
        Some(r"line 59\d:"),
        Some(0),
        Some(550),
        Some(604),
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("line 595:"));
    assert!(content.contains("line 599:"));
    assert!(content.contains("... (starting from line #550) ..."));
    assert!(content.contains("[Showing"));

    // Synthesized hunk header carries correct original-file line numbers,
    // seeded by `@@ -1,1000 +1,1000 @@` at line 4 — even though that line
    // sits outside the [550, 604] window. See the matching test in
    // `diff_file_tests.rs` for the full layout walkthrough.
    assert!(
        content.contains("@@ -1,0 +591,10 @@"),
        "expected accurate synthesized hunk header. content:\n{content}"
    );
}

#[test]
fn diff_commit_range_start_beyond_total_errors() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(10));

    let outcome = git_diff_commit_impl(
        dir.path(),
        "abc123",
        &["big.rs"],
        None,
        None,
        Some(999),
        None,
        &runner,
        &[],
    )
    .unwrap();

    assert!(outcome.into_content().is_none(), "expected error outcome");
}

#[test]
fn diff_commit_git_error() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::error("fatal: bad revision");

    let outcome = git_diff_commit_impl(
        dir.path(),
        "bad",
        &["file.rs"],
        None,
        None,
        None,
        None,
        &runner,
        &[],
    )
    .unwrap();
    assert!(outcome.into_content().is_none(), "expected error outcome");
}
