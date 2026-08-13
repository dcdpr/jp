use super::*;

#[test]
fn extract_window_marks_single_line() {
    let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7";
    let result = extract_window(content, 4, None);

    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();

    assert_eq!(marked.len(), 1);
    assert!(marked[0].contains("line4"));
}

#[test]
fn extract_window_marks_multiline_range() {
    let content = (1..=20)
        .map(|n| format!("line{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let result = extract_window(&content, 10, Some(5));

    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();

    // start_line=5, line=10 → 6 marked lines (5, 6, 7, 8, 9, 10).
    assert_eq!(marked.len(), 6);
    assert!(marked.iter().any(|l| l.contains("line5")));
    assert!(marked.iter().any(|l| l.contains("line10")));
    // line11 is in the trailing context window but should NOT be marked.
    assert!(
        result
            .lines()
            .filter(|l| l.contains("line11"))
            .any(|l| !l.trim_start().starts_with('>'))
    );
}

#[test]
fn extract_window_clamps_to_first_line() {
    let content = "line1\nline2\nline3";
    let result = extract_window(content, 1, None);
    assert!(
        result
            .lines()
            .any(|l| l.trim_start().starts_with('>') && l.contains("line1"))
    );
}

#[test]
fn extract_window_clamps_to_last_line() {
    let content = "line1\nline2\nline3";
    let result = extract_window(content, 3, None);
    assert!(
        result
            .lines()
            .any(|l| l.trim_start().starts_with('>') && l.contains("line3"))
    );
}

#[test]
fn extract_window_marks_two_line_range() {
    // Adjacent multi-line comment.
    let content = "a\nb\nc\nd\ne";
    let result = extract_window(content, 3, Some(2));

    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();

    assert_eq!(marked.len(), 2);
    assert!(marked.iter().any(|l| l.contains('b')));
    assert!(marked.iter().any(|l| l.contains('c')));
}

#[test]
fn extract_window_handles_empty_file() {
    assert_eq!(extract_window("", 1, None), "(empty file)");
}

// --- diff anchor validation ---

/// A patch shaped like the real-world failure this validation was added for: PR
/// \#849 changed `crates/jp_cli/src/lib.rs` with (among others) hunks `@@
/// -659,7 +694,33 @@` and `@@ -744,8 +769,16 @@`.
/// A comment anchored at RIGHT line 751 — between the hunks — was accepted by
/// GitHub but never displayed.
fn pr849_style_patch() -> DiffRanges {
    DiffRanges::from_patch(
        "@@ -659,7 +694,33 @@ fn run_inner(cli: Cli) {\n context\n+added\n context\n@@ -744,8 \
         +769,16 @@\n context\n-removed\n+added\n",
    )
}

#[test]
fn diff_ranges_parses_hunk_headers() {
    let ranges = pr849_style_patch();
    assert_eq!(ranges.left, vec![(659, 665), (744, 751)]);
    assert_eq!(ranges.right, vec![(694, 726), (769, 784)]);
}

#[test]
fn diff_ranges_zero_counts_produce_no_ranges() {
    // Pure addition: nothing commentable on the LEFT side.
    let added = DiffRanges::from_patch("@@ -0,0 +1,5 @@\n+a\n+b\n+c\n+d\n+e\n");
    assert_eq!(added.left, vec![]);
    assert_eq!(added.right, vec![(1, 5)]);

    // Pure removal (deleted file): nothing commentable on the RIGHT side.
    let removed = DiffRanges::from_patch("@@ -1,5 +0,0 @@\n-a\n-b\n-c\n-d\n-e\n");
    assert_eq!(removed.left, vec![(1, 5)]);
    assert_eq!(removed.right, vec![]);
}

#[test]
fn diff_ranges_bare_start_means_count_one() {
    let ranges = DiffRanges::from_patch("@@ -5 +7 @@\n-x\n+y\n");
    assert_eq!(ranges.left, vec![(5, 5)]);
    assert_eq!(ranges.right, vec![(7, 7)]);
}

#[test]
fn diff_ranges_ignores_hunk_body_content() {
    // `@@` only counts at the start of a header-shaped line; content lines
    // are prefixed with ` `/`+`/`-` and never match.
    let ranges = DiffRanges::from_patch("@@ -1,2 +1,2 @@\n let x = \"@@ -9,9 +9,9 @@\";\n+y\n");
    assert_eq!(ranges.left, vec![(1, 2)]);
    assert_eq!(ranges.right, vec![(1, 2)]);
}

#[test]
fn check_anchor_accepts_lines_inside_hunks() {
    let ranges = pr849_style_patch();

    // Single line, each side.
    assert!(check_anchor(&ranges, "f.rs", 694, None, Side::Right, None).is_ok());
    assert!(check_anchor(&ranges, "f.rs", 784, None, Side::Right, None).is_ok());
    assert!(check_anchor(&ranges, "f.rs", 661, None, Side::Left, None).is_ok());

    // Multi-line within one hunk.
    assert!(check_anchor(&ranges, "f.rs", 726, Some(700), Side::Right, None).is_ok());
}

#[test]
fn check_anchor_rejects_line_between_hunks_with_nearest() {
    // The exact PR #849 failure: RIGHT line 751 falls between the hunks.
    let ranges = pr849_style_patch();
    let err = check_anchor(
        &ranges,
        "crates/jp_cli/src/lib.rs",
        751,
        None,
        Side::Right,
        None,
    )
    .expect_err("751 is not commentable");

    // Distances: |751-726| = 25, |769-751| = 18 → nearest is 769.
    assert!(err.contains("Nearest commentable line: 769"), "{err}");
    assert!(err.contains("694-726, 769-784"), "{err}");
    assert!(err.contains("`line` (751, RIGHT)"), "{err}");
}

#[test]
fn check_anchor_validates_start_line_on_its_own_side() {
    let ranges = pr849_style_patch();

    // `start_side` LEFT: 660 is valid on LEFT even though it's not a valid
    // RIGHT anchor.
    assert!(
        check_anchor(
            &ranges,
            "f.rs",
            700,
            Some(660),
            Side::Right,
            Some(Side::Left)
        )
        .is_ok()
    );

    // Without an explicit `start_side`, the start falls back to `side`
    // (RIGHT), where 660 is invalid.
    let err = check_anchor(&ranges, "f.rs", 700, Some(660), Side::Right, None)
        .expect_err("660 is not a RIGHT anchor");
    assert!(err.contains("`start_line` (660, RIGHT)"), "{err}");
}

#[test]
fn check_anchor_reports_empty_side() {
    // Deleted file: no RIGHT anchors exist at all.
    let ranges = DiffRanges::from_patch("@@ -1,5 +0,0 @@\n-a\n");
    let err = check_anchor(&ranges, "gone.rs", 3, None, Side::Right, None)
        .expect_err("no RIGHT anchors in a deleted file");
    assert!(err.contains("(none)"), "{err}");
    assert!(!err.contains("Nearest"), "{err}");
}

#[test]
fn diff_ranges_nearest_clamps_into_ranges() {
    let ranges = pr849_style_patch();
    // Below every hunk → first hunk's start.
    assert_eq!(ranges.nearest(Side::Right, 1), Some(694));
    // Above every hunk → last hunk's end.
    assert_eq!(ranges.nearest(Side::Right, 9000), Some(784));
    // Inside a hunk → the line itself.
    assert_eq!(ranges.nearest(Side::Right, 700), Some(700));
}

#[test]
fn extract_window_handles_start_line_past_end() {
    // Validation should keep us out of this case in practice, but the
    // function shouldn't panic if it happens.
    let content = "a\nb\nc";
    let result = extract_window(content, 100, Some(50));
    // Falls back to clamping: line_idx = 2 (last), start_idx = 2.
    let marked: Vec<&str> = result
        .lines()
        .filter(|l| l.trim_start().starts_with('>'))
        .collect();
    assert_eq!(marked.len(), 1);
    assert!(marked[0].contains('c'));
}
