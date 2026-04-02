use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

fn single_file_diff() -> &'static str {
    "\
diff --git a/test.rs b/test.rs
index abc123..def456 100644
--- a/test.rs
+++ b/test.rs
@@ -1 +1 @@
-old line
+new line"
}

fn two_file_diff() -> &'static str {
    "\
diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,3 @@
 fn main() {
-    println!(\"hello\");
+    println!(\"world\");
 }
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-pub fn greet() {}
+pub fn greet(name: &str) {}"
}

/// Build a single-file diff with `line_count` added lines.
fn large_file_diff(line_count: usize) -> String {
    let mut lines = vec![
        "diff --git a/big.rs b/big.rs".to_string(),
        "--- a/big.rs".to_string(),
        "+++ b/big.rs".to_string(),
        "@@ -1,1000 +1,1000 @@".to_string(),
    ];

    for i in 0..line_count {
        lines.push(format!("+line {i}: generated content"));
    }

    lines.join("\n")
}

#[test]
fn parse_status_valid() {
    assert_eq!(DiffStatus::parse("staged").unwrap(), DiffStatus::Staged);
    assert_eq!(DiffStatus::parse("unstaged").unwrap(), DiffStatus::Unstaged);
}

#[test]
fn parse_status_rejects_all() {
    let err = DiffStatus::parse("all").unwrap_err();
    assert!(err.contains("all"), "error should mention the bad value");
}

#[test]
fn parse_status_invalid() {
    let err = DiffStatus::parse("bogus").unwrap_err();
    assert!(err.contains("bogus"), "error should mention the bad value");
}

#[test]
fn extract_path_normal() {
    assert_eq!(
        extract_path("diff --git a/src/main.rs b/src/main.rs"),
        "src/main.rs"
    );
}

#[test]
fn extract_path_no_b_prefix() {
    assert_eq!(extract_path("garbage"), "garbage");
}

#[test]
fn split_single_file() {
    let files = split_into_files(single_file_diff());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "test.rs");
    assert_eq!(files[0].line_count, 7);
}

#[test]
fn split_two_files() {
    let files = split_into_files(two_file_diff());
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "src/main.rs");
    assert_eq!(files[1].path, "src/lib.rs");
}

#[test]
fn split_empty_diff() {
    let files = split_into_files("");
    assert!(files.is_empty());
}

#[test]
fn format_small_diff_not_truncated() {
    let out = format_diff(single_file_diff(), MAX_LINES_PER_FILE);

    assert!(out.contains("Changed files:"));
    assert!(out.contains("test.rs (7 lines)"));
    assert!(!out.contains("truncated"));
    assert!(out.contains("```diff"));
    assert!(out.contains("-old line"));
    assert!(out.contains("+new line"));
}

#[test]
fn format_multi_file_diff() {
    let out = format_diff(two_file_diff(), MAX_LINES_PER_FILE);

    assert!(out.contains("src/main.rs"));
    assert!(out.contains("src/lib.rs"));
    assert!(out.contains("println"));
    assert!(out.contains("greet"));
}

#[test]
fn format_truncates_large_file() {
    let diff = large_file_diff(100);
    let out = format_diff(&diff, 20);

    assert!(out.contains("truncated to 20"));
    assert!(out.contains("[Truncated 20/104 lines for `big.rs`."));
    assert!(out.contains("Re-run with `paths`"));

    // Should contain exactly 20 lines of diff content inside the code fence
    let fence_content = out
        .split("```diff\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    assert_eq!(fence_content.lines().count(), 20);
}

#[test]
fn format_shows_trailing_note_when_truncated() {
    let diff = large_file_diff(100);
    let out = format_diff(&diff, 20);

    assert!(out.contains("Some files were truncated."));
}

#[test]
fn format_empty_diff() {
    assert_eq!(format_diff("", 50), "No changes.");
}

#[test]
fn unstaged_diff() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(single_file_diff());

    let content = git_diff_impl(
        dir.path(),
        &["test.rs".to_string()],
        DiffStatus::Unstaged,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("Changed files:"));
    assert!(content.contains("test.rs"));
    assert!(content.contains("-old line"));
    assert!(content.contains("+new line"));
}

#[test]
fn staged_diff() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(single_file_diff());

    let content = git_diff_impl(
        dir.path(),
        &["test.rs".to_string()],
        DiffStatus::Staged,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("test.rs"));
}

#[test]
fn empty_diff_output() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_diff_impl(dir.path(), &[], DiffStatus::Unstaged, &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert_eq!(content, "No changes.");
}
