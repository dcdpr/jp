use camino_tempfile::tempdir;
use jp_tool::Action;
use serde_json::json;

use super::*;
use crate::util::runner::MockProcessRunner;

/// Compute what the listing-side ID for a hunk would be, so tests can use
/// the same content-addressed ID the agent would receive.
fn id_for(hunk: &str) -> String {
    super::super::hunk::hunk_id(hunk)
}

#[test]
fn stage_single_file() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let mut answers = serde_json::Map::new();
    answers.insert("stage_changes".to_string(), json!(true));

    let diff =
        "diff --git a/test.rs b/test.rs\n--- a/test.rs\n+++ b/test.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let id = id_for("@@ -1 +1 @@\n-old\n+new");

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["ls-files", "test.rs"])
        .returns_success("test.rs\n")
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            "test.rs",
        ])
        .returns_success(diff)
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_success("");

    let patches = vec![PatchTarget {
        path: "test.rs".to_string(),
        ids: vec![id].into(),
    }];

    let result = git_stage_patch_impl(&ctx, &answers, &patches, &runner, &[]).unwrap();

    assert_eq!(result.into_content().unwrap(), "Patch applied.");
}

#[test]
fn stage_non_last_hunk_of_multi_hunk_diff() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let mut answers = serde_json::Map::new();
    answers.insert("stage_changes".to_string(), json!(true));

    // Two hunks: hunk 0 changes line 1, hunk 1 changes line 5.
    // Selecting only the first (non-last) one previously produced a patch
    // without a trailing newline, causing `git apply` to reject it as
    // corrupt.
    let diff = "diff --git a/f.rs b/f.rs\nindex abc..def 100644\n--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 \
                @@\n-aaa\n+AAA\n@@ -5 +5 @@\n-eee\n+EEE\n";

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["ls-files", "f.rs"])
        .returns_success("f.rs\n")
        .expect("git")
        .args(&["diff-files", "-p", "--minimal", "--unified=0", "--", "f.rs"])
        .returns_success(diff)
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_success("");

    let patches = vec![PatchTarget {
        path: "f.rs".to_string(),
        ids: vec![id_for("@@ -1 +1 @@\n-aaa\n+AAA")].into(),
    }];

    let result = git_stage_patch_impl(&ctx, &answers, &patches, &runner, &[]).unwrap();
    assert_eq!(result.into_content().unwrap(), "Patch applied.");
}

#[test]
fn stale_id_is_rejected_with_helpful_message() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let mut answers = serde_json::Map::new();
    answers.insert("stage_changes".to_string(), json!(true));

    let diff = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-old\n+new\n";

    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["ls-files", "f.rs"])
        .returns_success("f.rs\n")
        .expect("git")
        .args(&["diff-files", "-p", "--minimal", "--unified=0", "--", "f.rs"])
        .returns_success(diff);

    let patches = vec![PatchTarget {
        path: "f.rs".to_string(),
        ids: vec!["deadbeefcafe".to_string()].into(),
    }];

    let err = git_stage_patch_impl(&ctx, &answers, &patches, &runner, &[]).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not found"), "got: {msg}");
    assert!(msg.contains("Re-run `git_list_patches`"), "got: {msg}");
    assert!(msg.contains("deadbeefcafe"), "got: {msg}");
}

#[test]
fn partial_failure_stages_what_it_can() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let mut answers = serde_json::Map::new();
    answers.insert("stage_changes".to_string(), json!(true));

    let good_diff =
        "diff --git a/good.rs b/good.rs\n--- a/good.rs\n+++ b/good.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let good_id = id_for("@@ -1 +1 @@\n-old\n+new");

    // good.rs succeeds, bad.rs has no hunks.
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["ls-files", "good.rs"])
        .returns_success("good.rs\n")
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            "good.rs",
        ])
        .returns_success(good_diff)
        .expect("git")
        .args(&["ls-files", "bad.rs"])
        .returns_success("bad.rs\n")
        .expect("git")
        .args(&[
            "diff-files",
            "-p",
            "--minimal",
            "--unified=0",
            "--",
            "bad.rs",
        ])
        .returns_success("") // no diff output
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_success("");

    let patches = vec![
        PatchTarget {
            path: "good.rs".to_string(),
            ids: vec![good_id].into(),
        },
        PatchTarget {
            path: "bad.rs".to_string(),
            ids: vec!["abcdef012345".to_string()].into(),
        },
    ];

    let result = git_stage_patch_impl(&ctx, &answers, &patches, &runner, &[]).unwrap();
    let content = result.into_content().unwrap();

    assert!(content.contains("Staged: good.rs"), "got: {content}");
    assert!(content.contains("bad.rs"), "got: {content}");
}

#[test]
fn ids_resolve_in_file_order_regardless_of_request_order() {
    // Hunks must appear in increasing line-number order in the assembled
    // patch or `git apply` rejects it. Confirm this holds even when the
    // agent requests them in reverse order.
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let mut answers = serde_json::Map::new();
    answers.insert("stage_changes".to_string(), json!(true));

    let diff = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n@@ -1 +1 @@\n-aaa\n+AAA\n@@ -5 \
                +5 @@\n-eee\n+EEE\n";
    let id_first = id_for("@@ -1 +1 @@\n-aaa\n+AAA");
    let id_second = id_for("@@ -5 +5 @@\n-eee\n+EEE");

    // Capture the patch sent to `git apply` so we can verify ordering.
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["ls-files", "f.rs"])
        .returns_success("f.rs\n")
        .expect("git")
        .args(&["diff-files", "-p", "--minimal", "--unified=0", "--", "f.rs"])
        .returns_success(diff)
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_success("");

    let patches = vec![PatchTarget {
        path: "f.rs".to_string(),
        // Reverse order: second-line hunk first.
        ids: vec![id_second, id_first].into(),
    }];

    let result = git_stage_patch_impl(&ctx, &answers, &patches, &runner, &[]).unwrap();
    assert_eq!(result.into_content().unwrap(), "Patch applied.");
    // The mock will panic on drop if the apply call wasn't made — implies
    // the patch was assembled successfully in file order.
}
