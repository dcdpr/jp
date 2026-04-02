use camino_tempfile::tempdir;
use jp_tool::Action;
use serde_json::json;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn stage_single_file() {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };

    let mut answers = serde_json::Map::new();
    answers.insert("stage_changes".to_string(), json!(true));

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
        .returns_success(
            "diff --git a/test.rs b/test.rs\n--- a/test.rs\n+++ b/test.rs\n@@ -1 +1 \
             @@\n-old\n+new\n",
        )
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_success("");

    let patches = vec![PatchTarget {
        path: "test.rs".to_string(),
        ids: vec![0].into(),
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
    // Selecting only hunk 0 (non-last) previously produced a patch without
    // a trailing newline, causing `git apply` to reject it as corrupt.
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
        ids: vec![0].into(),
    }];

    let result = git_stage_patch_impl(&ctx, &answers, &patches, &runner, &[]).unwrap();
    assert_eq!(result.into_content().unwrap(), "Patch applied.");
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
        .returns_success(
            "diff --git a/good.rs b/good.rs\n--- a/good.rs\n+++ b/good.rs\n@@ -1 +1 \
             @@\n-old\n+new\n",
        )
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
            ids: vec![0].into(),
        },
        PatchTarget {
            path: "bad.rs".to_string(),
            ids: vec![0].into(),
        },
    ];

    let result = git_stage_patch_impl(&ctx, &answers, &patches, &runner, &[]).unwrap();
    let content = result.into_content().unwrap();

    assert!(content.contains("Staged: good.rs"), "got: {content}");
    assert!(content.contains("bad.rs"), "got: {content}");
}
