use super::*;

#[test]
fn hunk_id_is_stable() {
    let h = "@@ -1 +1 @@\n-old\n+new";
    assert_eq!(hunk_id(h), hunk_id(h));
}

#[test]
fn hunk_id_ignores_trailing_newline() {
    let a = "@@ -1 +1 @@\n-old\n+new";
    let b = "@@ -1 +1 @@\n-old\n+new\n";
    let c = "@@ -1 +1 @@\n-old\n+new\n\n";
    assert_eq!(hunk_id(a), hunk_id(b));
    assert_eq!(hunk_id(a), hunk_id(c));
}

#[test]
fn hunk_id_distinguishes_content() {
    let a = "@@ -1 +1 @@\n-old\n+new";
    let b = "@@ -1 +1 @@\n-old\n+different";
    assert_ne!(hunk_id(a), hunk_id(b));
}

#[test]
fn hunk_id_distinguishes_location() {
    // Same change, different line numbers — different IDs. This is
    // intentional: a hunk that targets line 1 versus line 100 should be
    // distinguishable, even with identical body text.
    let a = "@@ -1 +1 @@\n-old\n+new";
    let b = "@@ -100 +100 @@\n-old\n+new";
    assert_ne!(hunk_id(a), hunk_id(b));
}

#[test]
fn hunk_id_has_expected_length() {
    let id = hunk_id("@@ -1 +1 @@\n-old\n+new");
    assert_eq!(id.len(), HUNK_ID_HEX_LEN);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn split_hunks_preserves_order_and_headers() {
    let stdout = "diff --git a/f b/f\nindex abc..def 100644\n--- a/f\n+++ b/f\n@@ -1 +1 \
                  @@\n-a\n+A\n@@ -5 +5 @@\n-e\n+E\n";

    let hunks = split_hunks(stdout);
    assert_eq!(hunks.len(), 2);
    assert!(hunks[0].starts_with("@@ -1 +1 @@"));
    assert!(hunks[1].starts_with("@@ -5 +5 @@"));
}

#[test]
fn split_hunks_empty_for_no_diff() {
    assert!(split_hunks("").is_empty());
    assert!(split_hunks("diff --git a/f b/f\n--- a/f\n+++ b/f\n").is_empty());
}

#[test]
fn diff_header_returns_prefix_before_first_hunk() {
    let stdout =
        "diff --git a/f b/f\nindex abc..def 100644\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n-a\n+A\n";

    let header = diff_header(stdout).unwrap();
    assert_eq!(
        header,
        "diff --git a/f b/f\nindex abc..def 100644\n--- a/f\n+++ b/f"
    );
}

#[test]
fn diff_header_none_when_no_hunks() {
    assert!(diff_header("").is_none());
    assert!(diff_header("diff --git a/f b/f\n--- a/f\n+++ b/f\n").is_none());
}
