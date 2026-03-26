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
fn truncate_small_diff_unchanged() {
    let (content, note) = truncate_diff(small_diff(), MAX_LINES);

    assert_eq!(content, small_diff());
    assert!(note.is_none());
}

#[test]
fn truncate_large_diff() {
    let diff = large_diff(600);
    let (content, note) = truncate_diff(&diff, MAX_LINES);
    let note = note.expect("should have a note");

    assert_eq!(content.lines().count(), 500);
    assert!(note.contains("500/604"));
    assert!(note.contains("`pattern`"));
}

#[test]
fn grep_finds_matches() {
    let (content, _note) = grep_diff(small_diff(), "println", 1).unwrap();

    assert!(content.contains("println"));
    assert!(content.contains("hello"));
    assert!(content.contains("world"));
}

#[test]
fn grep_no_matches() {
    let (content, note) = grep_diff(small_diff(), "nonexistent_pattern", 3).unwrap();

    assert!(content.contains("No matches"));
    assert!(note.is_none());
}

#[test]
fn grep_context_controls_visible_lines() {
    // With 0 context, only matching lines are shown.
    let (content_0, _) = grep_diff(small_diff(), "hello", 0).unwrap();
    let lines_0: Vec<&str> = content_0
        .lines()
        .filter(|l| !l.starts_with('[') && !l.is_empty())
        .collect();

    // With 2 context, we get surrounding lines too.
    let (content_2, _) = grep_diff(small_diff(), "hello", 2).unwrap();
    let lines_2: Vec<&str> = content_2
        .lines()
        .filter(|l| !l.starts_with('[') && !l.is_empty())
        .collect();

    assert!(lines_2.len() >= lines_0.len());
}

#[test]
fn grep_separates_non_contiguous_regions() {
    // Build a diff with two matches far apart.
    let mut lines = vec!["diff --git a/f.rs b/f.rs".to_string()];
    lines.push("-match_first".to_string());
    for i in 0..20 {
        lines.push(format!(" filler line {i}"));
    }
    lines.push("+match_second".to_string());

    let diff = lines.join("\n");
    let (content, _) = grep_diff(&diff, "match_", 1).unwrap();

    assert!(content.contains("match_first"),);
    assert!(content.contains("match_second"),);
}

#[test]
fn grep_includes_file_and_hunk_headers() {
    let (content, _) = grep_diff(small_diff(), "world", 0).unwrap();

    // Even with 0 context, the diff --git and @@ headers should be present.
    assert!(content.contains("diff --git"), "missing header: {content}");
    assert!(content.contains("@@ "), "missing hunk header: {content}");
}

#[test]
fn grep_synthesizes_hunk_headers_with_line_numbers() {
    // Single hunk with two matches far apart — each region should
    // get a @@ header with the correct line number.
    let mut lines = vec![
        "diff --git a/f.rs b/f.rs".to_string(),
        "--- a/f.rs".to_string(),
        "+++ b/f.rs".to_string(),
        "@@ -1,30 +1,30 @@".to_string(),
    ];
    lines.push("+match_first".to_string());
    for i in 0..20 {
        lines.push(format!(" filler line {i}"));
    }
    lines.push("+match_second".to_string());

    let diff = lines.join("\n");
    let (content, _) = grep_diff(&diff, "match_", 0).unwrap();

    let hunk_count = content.matches("@@ ").count();
    assert!(
        hunk_count >= 2,
        "each region should have a @@ header, got {hunk_count}. content:\n{content}"
    );

    // First match is at new-file line 1, second at line 22.
    assert!(
        content.contains("-1,0 +1,1 @@"),
        "first region header. content:\n{content}"
    );
    assert!(
        content.contains("+22,1 @@"),
        "second region header. content:\n{content}"
    );
}

#[test]
fn parse_hunk_start_cases() {
    assert_eq!(parse_hunk_start("@@ -1,3 +1,3 @@"), (1, 1));
    assert_eq!(parse_hunk_start("@@ -0,0 +1,417 @@"), (0, 1));
    assert_eq!(parse_hunk_start("@@ -10,5 +42,7 @@ fn main()"), (10, 42));
    assert_eq!(parse_hunk_start("garbage"), (0, 0));
}

#[test]
fn grep_invalid_regex_errors() {
    let result = grep_diff(small_diff(), "[invalid", 0);
    assert!(result.is_err());
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
    assert!(content.ends_with("\n```"));
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
