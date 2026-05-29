use jp_github::models::{
    User,
    pulls::{Review, ReviewComment, ReviewState, Side},
    repos::{DiffEntry, DiffEntryStatus},
};

use super::*;

fn entry(
    filename: &str,
    status: DiffEntryStatus,
    additions: u64,
    deletions: u64,
    patch: Option<&str>,
) -> DiffEntry {
    DiffEntry {
        filename: filename.to_owned(),
        status,
        additions,
        deletions,
        changes: additions + deletions,
        previous_filename: None,
        patch: patch.map(str::to_owned),
    }
}

fn renamed_entry(prev: &str, new: &str, patch: Option<&str>) -> DiffEntry {
    DiffEntry {
        filename: new.to_owned(),
        status: DiffEntryStatus::Renamed,
        additions: 0,
        deletions: 0,
        changes: 0,
        previous_filename: Some(prev.to_owned()),
        patch: patch.map(str::to_owned),
    }
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn review(id: u64, login: &str, state: ReviewState, body: Option<&str>) -> Review {
    Review {
        id,
        node_id: format!("nid_{id}"),
        user: Some(User {
            login: login.to_owned(),
        }),
        body: body.map(str::to_owned),
        state,
        html_url: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn comment(
    id: u64,
    review_id: Option<u64>,
    path: &str,
    line: Option<u64>,
    side: Option<Side>,
    start_line: Option<u64>,
    start_side: Option<Side>,
    in_reply_to: Option<u64>,
    body: &str,
    login: &str,
) -> ReviewComment {
    ReviewComment {
        id,
        pull_request_review_id: review_id,
        path: path.to_owned(),
        line,
        side,
        start_line,
        start_side,
        original_line: line,
        original_side: side,
        original_start_line: start_line,
        original_start_side: start_side,
        in_reply_to_id: in_reply_to,
        body: body.to_owned(),
        user: Some(User {
            login: login.to_owned(),
        }),
        created_at: None,
        outdated: false,
    }
}

/// Builder for an outdated comment: GitHub clears the live `line` / `side`
/// fields but retains the `original_*` anchor, and the GraphQL `outdated` flag
/// is set.
#[allow(clippy::too_many_arguments)]
fn outdated_comment(
    id: u64,
    review_id: Option<u64>,
    path: &str,
    original_line: u64,
    original_side: Side,
    body: &str,
    login: &str,
) -> ReviewComment {
    ReviewComment {
        id,
        pull_request_review_id: review_id,
        path: path.to_owned(),
        line: None,
        side: None,
        start_line: None,
        start_side: None,
        original_line: Some(original_line),
        original_side: Some(original_side),
        original_start_line: None,
        original_start_side: None,
        in_reply_to_id: None,
        body: body.to_owned(),
        user: Some(User {
            login: login.to_owned(),
        }),
        created_at: None,
        outdated: true,
    }
}

#[test]
fn parses_canonical_pr_diff_uri() {
    let parsed = parse_uri(&url("gh://dcdpr/jp/pull/544/diff")).unwrap();
    assert_eq!(parsed.owner, "dcdpr");
    assert_eq!(parsed.repo, "jp");
    assert!(matches!(parsed.kind, ResourceKind::PullDiff {
        number: 544
    }));
    assert!(parsed.excludes.is_empty());
    assert!(!parsed.no_defaults);
}

#[test]
fn parses_shortform_pr_diff_uri() {
    let parsed = parse_uri(&url("gh:pull/544/diff")).unwrap();
    assert_eq!(parsed.owner, "dcdpr");
    assert_eq!(parsed.repo, "jp");
    assert!(matches!(parsed.kind, ResourceKind::PullDiff {
        number: 544
    }));
}

#[test]
fn parses_shortform_with_query_params() {
    let parsed = parse_uri(&url("gh:pull/42/diff?exclude=docs/**&no_defaults=true")).unwrap();
    assert_eq!(parsed.owner, "dcdpr");
    assert_eq!(parsed.repo, "jp");
    assert_eq!(parsed.excludes, vec!["docs/**"]);
    assert!(parsed.no_defaults);
}

#[test]
fn parses_canonical_pr_reviews_uri() {
    let parsed = parse_uri(&url("gh://dcdpr/jp/pull/544/reviews")).unwrap();
    assert!(matches!(parsed.kind, ResourceKind::PullReviews {
        number: 544
    }));
}

#[test]
fn parses_shortform_pr_reviews_uri() {
    let parsed = parse_uri(&url("gh:pull/544/reviews")).unwrap();
    assert_eq!(parsed.owner, "dcdpr");
    assert_eq!(parsed.repo, "jp");
    assert!(matches!(parsed.kind, ResourceKind::PullReviews {
        number: 544
    }));
}

#[test]
fn renders_empty_reviews_attachment() {
    let uri = url("gh:pull/42/reviews");
    let out = render_reviews(&uri, 42, &[], &[], 0);
    assert!(
        out.contains("PR #42 reviews: 0 submitted"),
        "missing header in:\n{out}"
    );
    assert!(
        out.contains("No reviews or inline comments yet"),
        "missing empty marker in:\n{out}"
    );
}

#[test]
fn renders_review_summaries_with_state_labels() {
    let uri = url("gh:pull/42/reviews");
    let reviews = vec![
        review(1, "alice", ReviewState::Approved, Some("first")),
        review(2, "bob", ReviewState::Commented, Some("second")),
    ];
    let out = render_reviews(&uri, 42, &reviews, &[], 0);
    assert!(out.contains("**alice** (submitted, approved)"), "{out}");
    assert!(out.contains("**bob** (submitted, comment)"), "{out}");
}

#[test]
fn renders_pending_reviews_with_login() {
    let uri = url("gh:pull/42/reviews");
    let reviews = vec![review(9, "someone", ReviewState::Pending, None)];
    let out = render_reviews(&uri, 42, &reviews, &[], 0);
    assert!(out.contains("1 pending (yours)"), "header in:\n{out}");
    assert!(
        out.contains("**someone** (pending)"),
        "pending review must show the author's login, not 'you':\n{out}"
    );
    assert!(
        !out.contains("**you**"),
        "the bare 'you' pronoun is misleading once a different model triages the same PR:\n{out}"
    );
}

#[test]
fn renders_inline_comments_grouped_by_file_and_line() {
    let uri = url("gh:pull/42/reviews");
    let reviews = vec![review(1, "alice", ReviewState::Commented, None)];
    let comments = vec![
        comment(
            100,
            Some(1),
            "src/lib.rs",
            Some(12),
            Some(Side::Right),
            None,
            None,
            None,
            "first comment",
            "alice",
        ),
        comment(
            101,
            Some(1),
            "src/lib.rs",
            Some(50),
            Some(Side::Right),
            Some(48),
            Some(Side::Right),
            None,
            "ranged",
            "alice",
        ),
        comment(
            102,
            Some(1),
            "src/main.rs",
            Some(5),
            Some(Side::Left),
            None,
            None,
            None,
            "on the old file",
            "alice",
        ),
    ];

    let out = render_reviews(&uri, 42, &reviews, &comments, 0);

    assert!(out.contains("## src/lib.rs"));
    assert!(out.contains("## src/main.rs"));
    assert!(out.contains("### Line 12 (RIGHT)"));
    assert!(out.contains("### Lines 48-50 (RIGHT)"));
    assert!(out.contains("### Line 5 (LEFT)"));
    assert!(out.contains("first comment") && out.contains("ranged") && out.contains("old file"));
}

#[test]
fn renders_replies_nested_under_parent() {
    let uri = url("gh:pull/42/reviews");
    let reviews = vec![review(1, "alice", ReviewState::Commented, None)];
    let comments = vec![
        comment(
            100,
            Some(1),
            "src/lib.rs",
            Some(12),
            Some(Side::Right),
            None,
            None,
            None,
            "original",
            "alice",
        ),
        comment(
            101,
            Some(1),
            "src/lib.rs",
            Some(12),
            Some(Side::Right),
            None,
            None,
            Some(100),
            "reply",
            "bob",
        ),
    ];

    let out = render_reviews(&uri, 42, &reviews, &comments, 0);
    let original = out.find("original").expect("original missing");
    let reply = out.find("reply").expect("reply missing");
    assert!(
        original < reply,
        "original should appear before reply:\n{out}"
    );
    assert!(
        out.contains("- **alice** (submitted, comment, id=100): original"),
        "top-level comment should expose its id:\n{out}"
    );
    assert!(
        out.contains("  - **bob** (reply, submitted, comment, id=101): reply"),
        "reply should be indented under its own bullet and expose its id:\n{out}"
    );
}

#[test]
fn renders_pending_inline_comment_with_login_and_id() {
    let uri = url("gh:pull/42/reviews");
    let reviews = vec![review(7, "someone", ReviewState::Pending, None)];
    let comments = vec![comment(
        200,
        Some(7),
        "src/lib.rs",
        Some(8),
        Some(Side::Right),
        None,
        None,
        None,
        "draft thought",
        "someone",
    )];

    let out = render_reviews(&uri, 42, &reviews, &comments, 0);
    assert!(
        out.contains("**someone** (pending, id=200): draft thought"),
        "pending inline should label author by login and surface the comment id:\n{out}"
    );
}

#[test]
fn rejects_wrong_scheme() {
    let err = parse_uri(&url("https://github.com/dcdpr/jp/pull/544"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("scheme"), "{err}");
}

#[test]
fn rejects_missing_segments() {
    let err = parse_uri(&url("gh://dcdpr/jp/pull/544"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unsupported"), "{err}");
}

/// `gh:/pull/N/diff` parses as a hostless URL with a path-absolute (NOT a
/// `cannot_be_a_base` URL).
/// Only the truly opaque `gh:pull/N/diff` is accepted as the shortform; the
/// path-absolute variant is undocumented and must be rejected so the URI
/// grammar stays explicit.
#[test]
fn rejects_path_absolute_hostless_shortform() {
    let err = parse_uri(&url("gh:/pull/544/diff"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unsupported"), "{err}");
}

#[test]
fn parses_exclude_query_param() {
    let parsed = parse_uri(&url("gh://dcdpr/jp/pull/1/diff?exclude=docs/**,*.md")).unwrap();
    assert_eq!(parsed.excludes, vec!["docs/**", "*.md"]);
    assert!(!parsed.no_defaults);
}

#[test]
fn parses_no_defaults_flag() {
    let parsed = parse_uri(&url("gh://dcdpr/jp/pull/1/diff?no_defaults=true")).unwrap();
    assert!(parsed.no_defaults);
}

#[test]
fn parses_include_outdated_flag() {
    let parsed = parse_uri(&url("gh:pull/42/reviews?include_outdated=true")).unwrap();
    assert!(parsed.include_outdated);

    let parsed = parse_uri(&url("gh:pull/42/reviews")).unwrap();
    assert!(
        !parsed.include_outdated,
        "include_outdated must default to false"
    );
}

#[test]
fn renders_outdated_comment_with_original_anchor_and_marker() {
    let uri = url("gh:pull/42/reviews?include_outdated=true");
    let reviews = vec![review(1, "alice", ReviewState::Commented, None)];
    let comments = vec![
        comment(
            100,
            Some(1),
            "src/lib.rs",
            Some(20),
            Some(Side::Right),
            None,
            None,
            None,
            "still here",
            "alice",
        ),
        outdated_comment(
            101,
            Some(1),
            "src/lib.rs",
            8,
            Side::Right,
            "used to be here",
            "alice",
        ),
    ];

    // `outdated_hidden = 0` because the caller chose to include them; the
    // outdated comment carries `outdated: true` directly on its struct,
    // so the renderer can mark its anchor accordingly.
    let out = render_reviews(&uri, 42, &reviews, &comments, 0);

    assert!(
        out.contains("### Line 20 (RIGHT)"),
        "live anchor should not be marked outdated:\n{out}"
    );
    assert!(
        out.contains("### Line 8 (RIGHT, outdated)"),
        "outdated comment should fall back to original_line and pick up the marker:\n{out}"
    );
    assert!(
        out.contains("**alice** (submitted, comment, outdated, id=101): used to be here"),
        "each outdated comment bullet should carry the marker in its label:\n{out}"
    );
    assert!(
        out.contains("**alice** (submitted, comment, id=100): still here"),
        "live comment bullets should be untouched:\n{out}"
    );
    assert!(
        !out.contains("hidden"),
        "no outdated should be hidden when caller passed them explicitly"
    );
}

#[test]
fn renders_outdated_marker_on_replies() {
    let uri = url("gh:pull/42/reviews?include_outdated=true");
    let reviews = vec![review(1, "alice", ReviewState::Commented, None)];

    // The whole thread is outdated (GraphQL flags every comment on the
    // thread). Both the top-level comment and its reply must carry the
    // marker so a reader scanning bullets in isolation sees it.
    let mut parent = outdated_comment(
        100,
        Some(1),
        "src/lib.rs",
        12,
        Side::Right,
        "original",
        "alice",
    );
    parent.in_reply_to_id = None;
    let mut child = outdated_comment(101, Some(1), "src/lib.rs", 12, Side::Right, "reply", "bob");
    child.in_reply_to_id = Some(100);

    let out = render_reviews(&uri, 42, &reviews, &[parent, child], 0);

    assert!(
        out.contains("- **alice** (submitted, comment, outdated, id=100): original"),
        "parent bullet should carry outdated marker:\n{out}"
    );
    assert!(
        out.contains("  - **bob** (reply, submitted, comment, outdated, id=101): reply"),
        "reply bullet should also carry outdated marker:\n{out}"
    );
}

#[test]
fn header_reports_count_when_outdated_comments_are_hidden() {
    let uri = url("gh:pull/42/reviews");
    let reviews = vec![review(1, "alice", ReviewState::Commented, None)];
    // Only the live comment in the slice; the outdated one is not passed.
    let comments = vec![comment(
        100,
        Some(1),
        "src/lib.rs",
        Some(20),
        Some(Side::Right),
        None,
        None,
        None,
        "still here",
        "alice",
    )];

    let out = render_reviews(&uri, 42, &reviews, &comments, 3);

    assert!(
        out.contains("3 outdated comment(s) hidden"),
        "header must surface hidden count:\n{out}"
    );
    assert!(
        out.contains("`?include_outdated=true`"),
        "header must point caller at the opt-in:\n{out}"
    );
}

#[test]
fn filter_drops_outdated_by_default() {
    let parsed = parse_uri(&url("gh:pull/42/reviews")).unwrap();
    let mut comments = vec![
        comment(
            100,
            Some(1),
            "src/lib.rs",
            Some(20),
            Some(Side::Right),
            None,
            None,
            None,
            "live",
            "alice",
        ),
        // GraphQL says id=101 is outdated; REST `line` happens to also
        // be null here (force-pushed shape). Filter must drop it.
        outdated_comment(
            101,
            Some(1),
            "src/lib.rs",
            8,
            Side::Right,
            "force-pushed out",
            "alice",
        ),
        // The bug from PR #594: a pending review comment that *looks*
        // outdated by REST shape (line=null, original_line=null) but
        // GraphQL reports as live (`outdated: false`). Filter must KEEP
        // it now that we read `outdated` directly off the comment.
        ReviewComment {
            id: 200,
            pull_request_review_id: Some(1),
            path: "src/lib.rs".to_owned(),
            line: None,
            side: None,
            start_line: None,
            start_side: None,
            original_line: None,
            original_side: None,
            original_start_line: None,
            original_start_side: None,
            in_reply_to_id: None,
            body: "pending but live".to_owned(),
            user: Some(User {
                login: "alice".to_owned(),
            }),
            created_at: None,
            outdated: false,
        },
    ];

    let hidden = apply_outdated_filter(&parsed, &mut comments);

    assert_eq!(hidden, 1, "only the GraphQL-flagged comment is dropped");
    assert_eq!(
        comments.len(),
        2,
        "the live comments survive even when REST anchor is missing"
    );
    let surviving_ids: Vec<u64> = comments.iter().map(|c| c.id).collect();
    assert!(surviving_ids.contains(&100));
    assert!(
        surviving_ids.contains(&200),
        "id=200 (pending, line=null, GraphQL says live) must NOT be filtered: {surviving_ids:?}"
    );
}

#[test]
fn filter_keeps_outdated_when_include_outdated_set() {
    let parsed = parse_uri(&url("gh:pull/42/reviews?include_outdated=true")).unwrap();
    let mut comments = vec![
        comment(
            100,
            Some(1),
            "src/lib.rs",
            Some(20),
            Some(Side::Right),
            None,
            None,
            None,
            "live",
            "alice",
        ),
        outdated_comment(
            101,
            Some(1),
            "src/lib.rs",
            8,
            Side::Right,
            "force-pushed out",
            "alice",
        ),
    ];

    let hidden = apply_outdated_filter(&parsed, &mut comments);

    assert_eq!(hidden, 0);
    assert_eq!(comments.len(), 2);
}

#[test]
fn outdated_anchor_uses_original_side_when_live_side_is_null() {
    let c = outdated_comment(
        7,
        Some(1),
        "src/main.rs",
        14,
        Side::Left,
        "removed line",
        "alice",
    );
    // `outdated_comment` sets `outdated: true` on the struct.
    assert_eq!(format_anchor(&c), "Line 14 (LEFT, outdated)");

    let mut live = c.clone();
    live.outdated = false;
    assert_eq!(format_anchor(&live), "Line 14 (LEFT)");
}

#[test]
fn rejects_unknown_query_param() {
    let err = parse_uri(&url("gh://dcdpr/jp/pull/1/diff?bogus=1"))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown query"), "{err}");
}

#[test]
fn filter_diff_drops_matching_files() {
    let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
index abc..def
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,1 +1,2 @@
-old
+new
diff --git a/snapshots/foo.snap b/snapshots/foo.snap
index 111..222
--- a/snapshots/foo.snap
+++ b/snapshots/foo.snap
@@ -1,1 +1,1 @@
-snap
+snap2
diff --git a/Cargo.toml b/Cargo.toml
index 333..444
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -1,1 +1,1 @@
-x = 1
+x = 2
";

    let patterns = compile_excludes(&[], false).unwrap();
    let (filtered, included, excluded) = filter_diff(diff, &patterns);

    assert_eq!(included, 2, "src/lib.rs and Cargo.toml should be kept");
    assert_eq!(excluded, 1, "snapshots/foo.snap should be filtered");
    assert!(filtered.contains("src/lib.rs"));
    assert!(filtered.contains("Cargo.toml"));
    assert!(
        !filtered.contains("snapshots/foo.snap"),
        "filtered diff should not mention excluded path:\n{filtered}",
    );
}

#[test]
fn filter_diff_user_excludes_compose_with_defaults() {
    let diff = "\
diff --git a/src/lib.rs b/src/lib.rs
index abc..def
@@ -1 +1 @@
-a
+b
diff --git a/docs/readme.md b/docs/readme.md
index 111..222
@@ -1 +1 @@
-x
+y
diff --git a/Cargo.lock b/Cargo.lock
index 333..444
@@ -1 +1 @@
-l
+l2
";

    // Default exclusions drop Cargo.lock; user adds docs/**.
    let patterns = compile_excludes(&["docs/**".to_owned()], false).unwrap();
    let (filtered, included, excluded) = filter_diff(diff, &patterns);

    assert_eq!(included, 1);
    assert_eq!(excluded, 2);
    assert!(filtered.contains("src/lib.rs"));
    assert!(!filtered.contains("docs/readme.md"));
    assert!(!filtered.contains("Cargo.lock"));
}

#[test]
fn filter_diff_no_defaults_skips_built_in_filters() {
    let diff = "\
diff --git a/snapshots/foo.snap b/snapshots/foo.snap
index 111..222
@@ -1 +1 @@
-x
+y
";

    // With no_defaults=true and no user excludes, nothing is filtered.
    let patterns = compile_excludes(&[], true).unwrap();
    let (filtered, included, excluded) = filter_diff(diff, &patterns);

    assert_eq!(included, 1);
    assert_eq!(excluded, 0);
    assert!(filtered.contains("snapshots/foo.snap"));
}

#[test]
fn synthesizes_unified_diff_from_files() {
    // Happy path: two modified files, both with patches. Output must
    // carry a `diff --git` header per file plus the patch verbatim, so
    // it can be consumed by anything that already parses unified diffs.
    let entries = vec![
        entry(
            "src/lib.rs",
            DiffEntryStatus::Modified,
            1,
            1,
            Some("@@ -1 +1 @@\n-old\n+new\n"),
        ),
        entry(
            "src/main.rs",
            DiffEntryStatus::Modified,
            1,
            0,
            Some("@@ -1,2 +1,3 @@\n a\n+inserted\n b\n"),
        ),
    ];

    let patterns = compile_excludes(&[], true).unwrap();
    let synth = synthesize_diff_from_files(&entries, &patterns);

    assert_eq!(synth.included, 2);
    assert_eq!(synth.excluded, 0);
    assert_eq!(synth.truncated, 0);
    assert_eq!(
        synth.text,
        "\
diff --git a/src/lib.rs b/src/lib.rs
@@ -1 +1 @@
-old
+new
diff --git a/src/main.rs b/src/main.rs
@@ -1,2 +1,3 @@
 a
+inserted
 b
"
    );
}

#[test]
fn synthesizer_filters_entries_before_emitting_patches() {
    // The exclude filter must drop matching entries entirely — not just
    // hide them in the output. This is the win over the diff-endpoint
    // path: we never have to look at a 5000-line lockfile patch to
    // decide we don't want it.
    let entries = vec![
        entry(
            "src/lib.rs",
            DiffEntryStatus::Modified,
            1,
            1,
            Some("@@ -1 +1 @@\n-a\n+b\n"),
        ),
        entry(
            "Cargo.lock",
            DiffEntryStatus::Modified,
            5000,
            5000,
            Some("@@ -1 +1 @@\n-x\n+y\n"),
        ),
        entry(
            "snapshots/foo.snap",
            DiffEntryStatus::Modified,
            10,
            10,
            Some("@@ -1 +1 @@\n-s\n+s2\n"),
        ),
    ];

    let patterns = compile_excludes(&[], false).unwrap();
    let synth = synthesize_diff_from_files(&entries, &patterns);

    assert_eq!(synth.included, 1);
    assert_eq!(synth.excluded, 2);
    assert!(synth.text.contains("diff --git a/src/lib.rs b/src/lib.rs"));
    assert!(
        !synth.text.contains("Cargo.lock"),
        "excluded file must not appear in output:\n{}",
        synth.text
    );
    assert!(
        !synth.text.contains("snapshots/foo.snap"),
        "excluded file must not appear in output:\n{}",
        synth.text
    );
}

#[test]
fn synthesizer_emits_placeholder_for_missing_patch() {
    // GitHub omits `patch` for files too large or binary. We still emit
    // the `diff --git` header so the LLM can see the file changed, plus
    // a placeholder summarizing the status and line counts.
    let entries = vec![entry(
        "assets/logo.png",
        DiffEntryStatus::Modified,
        0,
        0,
        None,
    )];

    let patterns = compile_excludes(&[], true).unwrap();
    let synth = synthesize_diff_from_files(&entries, &patterns);

    assert_eq!(synth.included, 1);
    assert_eq!(synth.truncated, 1);
    assert!(
        synth
            .text
            .contains("diff --git a/assets/logo.png b/assets/logo.png")
    );
    assert!(
        synth.text.contains("[patch omitted by GitHub"),
        "placeholder line should mention GitHub omitted the patch:\n{}",
        synth.text
    );
    assert!(
        synth.text.contains("status=modified"),
        "placeholder must surface the change status so reviewers know what kind of edit it \
         was:\n{}",
        synth.text
    );
}

#[test]
fn synthesizer_emits_rename_markers() {
    // Renames need the `rename from`/`rename to` markers AND the `a/`
    // path must point at the previous filename, otherwise downstream
    // consumers can't distinguish a rename from a delete+add. With a
    // patch attached, the markers come before the hunks.
    let entries = vec![
        renamed_entry("src/old_name.rs", "src/new_name.rs", None),
        renamed_entry(
            "src/touched_old.rs",
            "src/touched_new.rs",
            Some("@@ -1 +1 @@\n-a\n+b\n"),
        ),
    ];

    let patterns = compile_excludes(&[], true).unwrap();
    let synth = synthesize_diff_from_files(&entries, &patterns);

    assert_eq!(synth.included, 2);
    assert!(
        synth
            .text
            .contains("diff --git a/src/old_name.rs b/src/new_name.rs"),
        "a/ side must use the previous filename for renames:\n{}",
        synth.text
    );
    assert!(synth.text.contains("rename from src/old_name.rs"));
    assert!(synth.text.contains("rename to src/new_name.rs"));
    assert!(
        synth
            .text
            .contains("rename from src/touched_old.rs\nrename to src/touched_new.rs\n@@ "),
        "rename markers must appear before the hunks:\n{}",
        synth.text
    );
}

#[test]
fn synthesizer_returns_empty_for_no_entries() {
    // Defensive: a PR with no changed files (rare, but legal — e.g.
    // a force-push to identical content) must produce no body and zero
    // counters, not a malformed blob.
    let patterns = compile_excludes(&[], true).unwrap();
    let synth = synthesize_diff_from_files(&[], &patterns);

    assert_eq!(synth.included, 0);
    assert_eq!(synth.excluded, 0);
    assert_eq!(synth.truncated, 0);
    assert!(synth.text.is_empty());
}
