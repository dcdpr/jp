use super::*;

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
    let (content, note) = truncate_diff(small_diff(), 500);

    assert_eq!(content, small_diff());
    assert!(note.is_none());
}

#[test]
fn truncate_large_diff() {
    let diff = large_diff(600);
    let (content, note) = truncate_diff(&diff, 500);
    let note = note.expect("should have a note");

    assert_eq!(content.lines().count(), 500);
    assert!(note.contains("500/604"));
    assert!(note.contains("`pattern`"));
}

#[test]
fn grep_finds_matches() {
    let (content, _note) = grep_diff(small_diff(), "println", 1, None).unwrap();

    assert!(content.contains("println"));
    assert!(content.contains("hello"));
    assert!(content.contains("world"));
}

#[test]
fn grep_no_matches() {
    let (content, note) = grep_diff(small_diff(), "nonexistent_pattern", 3, None).unwrap();

    assert!(content.contains("No matches"));
    assert!(note.is_none());
}

#[test]
fn grep_context_controls_visible_lines() {
    // With 0 context, only matching lines are shown.
    let (content_0, _) = grep_diff(small_diff(), "hello", 0, None).unwrap();
    let lines_0: Vec<&str> = content_0
        .lines()
        .filter(|l| !l.starts_with('[') && !l.is_empty())
        .collect();

    // With 2 context, we get surrounding lines too.
    let (content_2, _) = grep_diff(small_diff(), "hello", 2, None).unwrap();
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
    let (content, _) = grep_diff(&diff, "match_", 1, None).unwrap();

    assert!(content.contains("match_first"),);
    assert!(content.contains("match_second"),);
}

#[test]
fn grep_includes_file_and_hunk_headers() {
    let (content, _) = grep_diff(small_diff(), "world", 0, None).unwrap();

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
    let (content, _) = grep_diff(&diff, "match_", 0, None).unwrap();

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
    let result = grep_diff(small_diff(), "[invalid", 0, None);
    assert!(result.is_err());
}

#[test]
fn grep_with_bounds_ignores_matches_outside_window() {
    // Layout (1-based input line numbers):
    //   1   diff --git a/f.rs b/f.rs
    //   2   --- a/f.rs
    //   3   +++ b/f.rs
    //   4   @@ -1,30 +1,30 @@
    //   5   +first
    //   6–25 “filler 0” … “filler 19”
    //   26  +second
    let mut lines = vec![
        "diff --git a/f.rs b/f.rs".to_string(),
        "--- a/f.rs".to_string(),
        "+++ b/f.rs".to_string(),
        "@@ -1,30 +1,30 @@".to_string(),
    ];
    lines.push("+first".to_string());
    for i in 0..20 {
        lines.push(format!(" filler {i}"));
    }
    lines.push("+second".to_string());

    let diff = lines.join("\n");

    // Window covers only the second match (line 26).
    let (content, _) = grep_diff(&diff, "first|second", 0, Some((26, 26))).unwrap();

    assert!(content.contains("+second"), "missing +second in: {content}");
    assert!(
        !content.contains("+first"),
        "+first leaked through bounds: {content}"
    );
}

#[test]
fn grep_with_bounds_synthesizes_correct_hunk_header() {
    // Construct a diff where the only match sits past the seeding @@ header,
    // and the bounds window starts *after* that @@ header. Without bounds-
    // aware structural tracking the synthesized header would emit `+0,*`.
    let mut lines = vec![
        "diff --git a/f.rs b/f.rs".to_string(),
        "--- a/f.rs".to_string(),
        "+++ b/f.rs".to_string(),
        "@@ -1,100 +1,100 @@".to_string(),
    ];
    for i in 0..50 {
        lines.push(format!(" filler {i}"));
    }
    lines.push("+target".to_string()); // input line 55, new_line at this point: 51.
    for i in 50..60 {
        lines.push(format!(" filler {i}"));
    }

    let diff = lines.join("\n");

    // Window starts at line 30, well past the seeding @@ header at line 4.
    let (content, _) = grep_diff(&diff, "^\\+target", 0, Some((30, 60))).unwrap();

    assert!(content.contains("+target"), "missing match: {content}");
    // "+target" sits in a context of unchanged ` filler` lines. By line 55,
    // 50 ` ` lines have advanced both old_line and new_line to 51.
    assert!(
        content.contains("@@ -51,0 +51,1 @@"),
        "expected accurate synthesized hunk header. content:\n{content}"
    );
}

#[test]
fn grep_with_bounds_clamps_context_to_window() {
    // 3-line file (after headers): one match flanked by filler. Asking for
    // 5 lines of context would normally pull both filler lines, but a tight
    // window must clamp the context.
    let lines = [
        "diff --git a/f.rs b/f.rs",
        "--- a/f.rs",
        "+++ b/f.rs",
        "@@ -1,3 +1,3 @@",
        " before",   // line 5
        "+match_me", // line 6
        " after",    // line 7
    ];

    let diff = lines.join("\n");

    // Window covers only the match itself (line 6).
    let (content, _) = grep_diff(&diff, "match_me", 5, Some((6, 6))).unwrap();

    assert!(content.contains("+match_me"));
    assert!(
        !content.contains(" before"),
        "context bled before window: {content}"
    );
    assert!(
        !content.contains(" after"),
        "context bled after window: {content}"
    );
}

#[test]
fn validate_line_range_accepts_valid() {
    assert!(validate_line_range(None, None).is_ok());
    assert!(validate_line_range(Some(1), None).is_ok());
    assert!(validate_line_range(None, Some(100)).is_ok());
    assert!(validate_line_range(Some(1), Some(100)).is_ok());
    assert!(validate_line_range(Some(50), Some(50)).is_ok());
}

#[test]
fn validate_line_range_rejects_zero() {
    assert!(validate_line_range(Some(0), None).is_err());
    assert!(validate_line_range(None, Some(0)).is_err());
    assert!(validate_line_range(Some(0), Some(0)).is_err());
}

#[test]
fn validate_line_range_rejects_inverted() {
    let err = validate_line_range(Some(50), Some(10)).unwrap_err();
    assert!(err.contains("less than or equal"));
}

#[test]
fn slice_diff_no_range_returns_input_unchanged() {
    let out = slice_diff(small_diff(), None, None);
    assert_eq!(out, small_diff());
}

#[test]
fn slice_diff_only_start_keeps_tail() {
    // small_diff line layout (1-based):
    //   1: diff --git ...
    //   2: index abc..def 100644
    //   3: --- a/src/main.rs
    //   4: +++ b/src/main.rs
    //   5: @@ -1,3 +1,3 @@
    //   6:  fn main() {
    //   7: -    println!("hello");
    //   8: +    println!("world");
    //   9:  }
    let out = slice_diff(small_diff(), Some(5), None);
    assert!(out.starts_with("@@ -1,3 +1,3 @@\n"));
    assert!(out.contains("+    println!(\"world\")"));
    assert!(!out.contains("diff --git"));
    assert!(!out.contains("--- a/src/main.rs"));
}

#[test]
fn slice_diff_only_end_keeps_head() {
    let out = slice_diff(small_diff(), None, Some(3));
    assert!(out.contains("diff --git"));
    assert!(out.contains("index abc..def"));
    assert!(out.contains("--- a/src/main.rs"));
    assert!(!out.contains("+++ b/src/main.rs"));
    assert!(!out.contains("@@"));
}

#[test]
fn slice_diff_both_bounds() {
    let out = slice_diff(small_diff(), Some(3), Some(5));
    // Lines 3..=5: --- a/..., +++ b/..., @@ ...
    assert_eq!(out.lines().count(), 3);
    assert!(out.contains("--- a/src/main.rs"));
    assert!(out.contains("+++ b/src/main.rs"));
    assert!(out.contains("@@ -1,3 +1,3 @@"));
}

#[test]
fn slice_diff_end_beyond_total_caps_silently() {
    // small_diff has 9 lines; asking for 1..=999 should give the whole diff,
    // no error.
    let out = slice_diff(small_diff(), Some(1), Some(999));
    assert_eq!(out, small_diff());
}

#[test]
fn add_slice_markers_wraps_content() {
    let mut content = "the body".to_string();
    add_slice_markers(&mut content, Some(50), Some(100));
    assert_eq!(
        content,
        "... (starting from line #50) ...\nthe body\n... (truncated after line #100) ..."
    );
}

#[test]
fn add_slice_markers_only_start() {
    let mut content = "body".to_string();
    add_slice_markers(&mut content, Some(7), None);
    assert_eq!(content, "... (starting from line #7) ...\nbody");
}

#[test]
fn add_slice_markers_only_end() {
    let mut content = "body".to_string();
    add_slice_markers(&mut content, None, Some(42));
    assert_eq!(content, "body\n... (truncated after line #42) ...");
}

#[test]
fn add_slice_markers_no_range_is_noop() {
    let mut content = "body".to_string();
    add_slice_markers(&mut content, None, None);
    assert_eq!(content, "body");
}
