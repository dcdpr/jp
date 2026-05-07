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
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("[Showing 500/604 lines."));
    // Truncation note now mentions both escape hatches.
    assert!(content.contains("`pattern`"));
    assert!(content.contains("`start_line`"));
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
        None,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("line 42:"));
    // Pattern mode emits the matches-note, not the truncation note.
    assert!(content.contains("[Showing"));
    assert!(!content.contains("Use `pattern` to search"));
}

#[test]
fn range_bypasses_truncation_cap() {
    let dir = tempdir().unwrap();
    // 600 content lines + 4 header lines = 604 total. Cap is 500.
    let runner = MockProcessRunner::success(large_diff(600));

    // Ask for a window that extends past the 500-line cap.
    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
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

    // Range markers present, no truncation note.
    assert!(content.contains("... (starting from line #400) ..."));
    assert!(content.contains("... (truncated after line #600) ..."));
    assert!(!content.contains("[Showing"));
    // Lines past the 500-line cap must be present.
    assert!(content.contains("line 500:"));
    assert!(content.contains("line 595:"));
}

#[test]
fn range_only_start_line() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(600));

    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
        &["big.rs"],
        None,
        None,
        Some(550),
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("... (starting from line #550) ..."));
    assert!(!content.contains("... (truncated after"));
    // From line 550 of output (4 header lines + content lines), we're at
    // diff content line ~546. Should have lines past 500.
    assert!(content.contains("line 595:"));
}

#[test]
fn range_with_pattern_slices_then_greps() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(600));

    // Pattern matches `line 5:`, `line 50:`...`line 599:`. The slice
    // restricts to a window where only some of those appear.
    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
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

    // Matches inside the window are present.
    assert!(content.contains("line 595:"));
    assert!(content.contains("line 599:"));
    // The slice markers are still in the diff content (above the matches).
    assert!(content.contains("... (starting from line #550) ..."));
    // Grep's matches note (operating on the slice) is present.
    assert!(content.contains("[Showing"));
}

#[test]
fn range_start_beyond_total_errors() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(large_diff(10));

    // Diff has only 14 lines (4 header + 10 content); start_line=999 is past
    // the end.
    let outcome = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
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
fn empty_diff() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_diff_file_impl(
        dir.path(),
        DiffStatus::Unstaged,
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
        None,
        None,
        &runner,
        &[],
    )
    .unwrap();

    assert!(outcome.into_content().is_none(), "expected error outcome");
}
