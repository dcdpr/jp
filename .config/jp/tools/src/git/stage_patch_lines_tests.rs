use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn parse_hunk_header_simple() {
    let h = parse_hunk_header("@@ -5 +5 @@").unwrap();
    assert_eq!(h.old_start, 5);
}

#[test]
fn parse_hunk_header_with_counts() {
    let h = parse_hunk_header("@@ -5,3 +5,2 @@").unwrap();
    assert_eq!(h.old_start, 5);
}

#[test]
fn parse_hunk_header_zero_count() {
    let h = parse_hunk_header("@@ -5,0 +5,3 @@").unwrap();
    assert_eq!(h.old_start, 5);
}

#[test]
fn parse_hunk_simple_replacement() {
    let hunk = "@@ -5 +5 @@\n-old\n+new";
    let (header, lines) = parse_hunk(hunk).unwrap();

    assert_eq!(header.old_start, 5);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].kind, DiffLineKind::Removal);
    assert_eq!(lines[0].content, "old");
    assert_eq!(lines[1].kind, DiffLineKind::Addition);
    assert_eq!(lines[1].content, "new");
}

#[test]
fn sub_hunk_select_all() {
    let hunk = "@@ -5,2 +5,2 @@\n-old1\n-old2\n+new1\n+new2";
    let result = build_sub_hunk(hunk, &[0, 1, 2, 3]).unwrap();
    assert_eq!(result, "@@ -5,2 +5,2 @@\n-old1\n-old2\n+new1\n+new2\n");
}

#[test]
fn sub_hunk_select_first_pair() {
    // [0] -old1  (old line 5)
    // [1] -old2  (old line 6)
    // [2] +new1
    // [3] +new2
    // Select [0, 2]: removal of old1 + addition of new1.
    let hunk = "@@ -5,2 +5,2 @@\n-old1\n-old2\n+new1\n+new2";
    let result = build_sub_hunk(hunk, &[0, 2]).unwrap();
    assert_eq!(result, "@@ -5,1 +5,1 @@\n-old1\n+new1\n");
}

#[test]
fn sub_hunk_select_second_pair() {
    let hunk = "@@ -5,2 +5,2 @@\n-old1\n-old2\n+new1\n+new2";
    let result = build_sub_hunk(hunk, &[1, 3]).unwrap();
    // First selected removal is old2 at old line 6.
    assert_eq!(result, "@@ -6,1 +6,1 @@\n-old2\n+new2\n");
}

#[test]
fn sub_hunk_pure_addition() {
    let hunk = "@@ -0,0 +1,3 @@\n+line1\n+line2\n+line3";
    let result = build_sub_hunk(hunk, &[0, 1]).unwrap();
    assert_eq!(result, "@@ -0,0 +0,2 @@\n+line1\n+line2\n");
}

#[test]
fn sub_hunk_pure_removal() {
    let hunk = "@@ -5,2 +5,0 @@\n-old1\n-old2";
    let result = build_sub_hunk(hunk, &[0]).unwrap();
    assert_eq!(result, "@@ -5,1 +5,0 @@\n-old1\n");
}

#[test]
fn sub_hunk_line_out_of_range() {
    let hunk = "@@ -5 +5 @@\n-old\n+new";
    let result = build_sub_hunk(hunk, &[5]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("out of range"));
}

#[test]
fn sub_hunk_duplicates_are_deduped() {
    let hunk = "@@ -5 +5 @@\n-old\n+new";
    let result = build_sub_hunk(hunk, &[0, 0, 1, 1]).unwrap();
    assert_eq!(result, "@@ -5,1 +5,1 @@\n-old\n+new\n");
}

#[test]
fn fetch_hunk_produces_valid_header() {
    let diff_output = indoc::indoc! {"
        diff --git a/test.rs b/test.rs
        index abc..def 100644
        --- a/test.rs
        +++ b/test.rs
        @@ -1 +1 @@
        -old
        +new
    "};

    let runner = MockProcessRunner::success(diff_output);
    let hunk = fetch_hunk("/tmp".into(), "test.rs", 0, &runner).unwrap();

    assert!(hunk.starts_with("@@ -"), "hunk header was: {hunk}");

    let (header, lines) = parse_hunk(&hunk).unwrap();
    assert_eq!(header.old_start, 1);
    assert_eq!(lines.len(), 2);
}

#[test]
fn fetch_hunk_second_of_two() {
    let diff_output = indoc::indoc! {"
        diff --git a/test.rs b/test.rs
        index abc..def 100644
        --- a/test.rs
        +++ b/test.rs
        @@ -1 +1 @@
        -a
        +A
        @@ -5 +5 @@
        -e
        +E
    "};

    let runner = MockProcessRunner::success(diff_output);
    let hunk = fetch_hunk("/tmp".into(), "test.rs", 1, &runner).unwrap();

    let (header, lines) = parse_hunk(&hunk).unwrap();
    assert_eq!(header.old_start, 5);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].content, "e");
    assert_eq!(lines[1].content, "E");
}
