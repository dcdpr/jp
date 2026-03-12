//! Integration tests for git tools against real git binary.
//!
//! Every test calls `tools::run()` with the same `Context` and `Tool` structs
//! that production uses, ensuring we test the full public interface.

use std::{fs, process::Command};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use jp_tool::{Action, Context, Outcome};
use serde_json::{Map, Value, json};
use tools::Tool;

fn has_git() -> bool {
    which::which("git").is_ok()
}

fn init_repo() -> (Utf8TempDir, Utf8PathBuf) {
    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();

    git(&root, &["init"]);
    git(&root, &["config", "user.email", "test@test.com"]);
    git(&root, &["config", "user.name", "Test"]);

    fs::write(root.join(".gitkeep"), "").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "init"]);

    (dir, root)
}

/// Run a raw git command, isolated from system config.
fn git(root: &Utf8Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "")
        .env("GIT_CONFIG_SYSTEM", "")
        .output()
        .unwrap();

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("git {args:?} failed: {stderr}");
    }

    String::from_utf8(output.stdout).unwrap()
}

fn staged_content(root: &Utf8Path, path: &str) -> String {
    let output = Command::new("git")
        .args(["show", &format!(":{path}")])
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "")
        .env("GIT_CONFIG_SYSTEM", "")
        .output()
        .unwrap();

    String::from_utf8(output.stdout).unwrap()
}

fn ctx(root: &Utf8Path) -> Context {
    Context {
        root: root.to_owned(),
        action: Action::Run,
    }
}

fn tool(name: &str, arguments: &Value) -> Tool {
    Tool {
        name: name.to_string(),
        arguments: arguments.as_object().unwrap().clone(),
        answers: Map::new(),
    }
}

fn tool_with_answers(name: &str, arguments: &Value, answers: &Value) -> Tool {
    Tool {
        name: name.to_string(),
        arguments: arguments.as_object().unwrap().clone(),
        answers: answers.as_object().unwrap().clone(),
    }
}

/// Call `tools::run` and return the success content, panicking on error.
async fn run_ok(ctx: Context, t: Tool) -> String {
    let name = t.name.clone();
    let outcome = tools::run(ctx, t).await.unwrap();
    match outcome {
        Outcome::Success { content } => content,
        other => panic!("{name} did not succeed: {other:?}"),
    }
}

/// Call `tools::run` and return the Outcome directly.
async fn run_outcome(ctx: Context, t: Tool) -> Outcome {
    tools::run(ctx, t).await.unwrap()
}

/// Commit a file with given content, then modify it in the working tree.
fn commit_then_modify(root: &Utf8Path, path: &str, original: &str, modified: &str) {
    fs::write(root.join(path), original).unwrap();
    git(root, &["add", path]);
    git(root, &["commit", "-m", &format!("add {path}")]);
    fs::write(root.join(path), modified).unwrap();
}

// --- git_add_intent ---

#[tokio::test]
async fn add_intent_marks_untracked_file() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("new.rs"), "fn main() {}\n").unwrap();

    let content = run_ok(
        ctx(&root),
        tool("git_add_intent", &json!({"paths": ["new.rs"]})),
    )
    .await;

    assert!(content.contains("1 file"));
    assert!(content.contains("intent-to-add"));

    // Verify the file is now visible to diff-files.
    let diff = git(&root, &["diff-files", "--name-only"]);
    assert!(diff.contains("new.rs"));
}

#[tokio::test]
async fn add_intent_multiple_files() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("a.rs"), "a\n").unwrap();
    fs::write(root.join("b.rs"), "b\n").unwrap();

    let content = run_ok(
        ctx(&root),
        tool("git_add_intent", &json!({"paths": ["a.rs", "b.rs"]})),
    )
    .await;

    assert!(content.contains("2 files"));
}

// --- git_list_patches ---

#[tokio::test]
async fn list_patches_shows_hunks_with_line_indices() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "f.rs", "aaa\nbbb\n", "AAA\nbbb\n");

    let content = run_ok(
        ctx(&root),
        tool("git_list_patches", &json!({"files": ["f.rs"]})),
    )
    .await;

    assert!(content.contains("<path>f.rs</path>"));
    assert!(content.contains("<id>0</id>"));
    assert!(content.contains("[0] -aaa"));
    assert!(content.contains("[1] +AAA"));
}

#[tokio::test]
async fn list_patches_multiple_hunks() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "m.rs", "a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");

    let content = run_ok(
        ctx(&root),
        tool("git_list_patches", &json!({"files": ["m.rs"]})),
    )
    .await;

    assert!(content.contains("<id>0</id>"));
    assert!(content.contains("<id>1</id>"));
    assert!(content.contains("[0] -a"));
    assert!(content.contains("[1] +A"));
    assert!(content.contains("[0] -e"));
    assert!(content.contains("[1] +E"));
}

#[tokio::test]
async fn list_patches_no_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("clean.rs"), "unchanged\n").unwrap();
    git(&root, &["add", "clean.rs"]);
    git(&root, &["commit", "-m", "add clean"]);

    let content = run_ok(
        ctx(&root),
        tool("git_list_patches", &json!({"files": ["clean.rs"]})),
    )
    .await;

    // No patches, just the empty wrapper.
    assert!(!content.contains("<id>"));
}

#[tokio::test]
async fn list_patches_missing_file_warns() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "real.rs", "old\n", "new\n");

    let content = run_ok(
        ctx(&root),
        tool(
            "git_list_patches",
            &json!({"files": ["ghost.rs", "real.rs"]}),
        ),
    )
    .await;

    assert!(content.contains("File not found: ghost.rs"));
    assert!(content.contains("<path>real.rs</path>"));
}

#[tokio::test]
async fn list_patches_intent_to_add_file() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("new.rs"), "line1\nline2\n").unwrap();

    run_ok(
        ctx(&root),
        tool("git_add_intent", &json!({"paths": ["new.rs"]})),
    )
    .await;

    let content = run_ok(
        ctx(&root),
        tool("git_list_patches", &json!({"files": ["new.rs"]})),
    )
    .await;

    assert!(content.contains("+line1"));
    assert!(content.contains("+line2"));
}

#[tokio::test]
async fn list_patches_deleted_file() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("doomed.rs"), "line1\nline2\nline3\n").unwrap();
    git(&root, &["add", "doomed.rs"]);
    git(&root, &["commit", "-m", "add doomed"]);

    // Delete the file in the working tree.
    fs::remove_file(root.join("doomed.rs")).unwrap();

    let content = run_ok(
        ctx(&root),
        tool("git_list_patches", &json!({"files": ["doomed.rs"]})),
    )
    .await;

    // All lines show as removals.
    assert!(content.contains("-line1"));
    assert!(content.contains("-line2"));
    assert!(content.contains("-line3"));
    // No "File not found" warning — git knows about this file.
    assert!(!content.contains("not found"));
}

#[tokio::test]
async fn stage_deleted_file_via_stage_patch() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("bye.rs"), "content\n").unwrap();
    git(&root, &["add", "bye.rs"]);
    git(&root, &["commit", "-m", "add bye"]);

    fs::remove_file(root.join("bye.rs")).unwrap();

    // Stage the deletion.
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "bye.rs", "ids": [0]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    // Verify the file is staged as deleted.
    let status = git(&root, &["status", "--porcelain"]);
    assert!(
        status.contains("D  bye.rs"),
        "expected 'D  bye.rs' in status, got: {status:?}"
    );
}

#[tokio::test]
async fn stage_patch_single_hunk() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "s.rs", "old\n", "new\n");

    let content = run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "s.rs", "ids": [0]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(content, "Patch applied.");
    assert_eq!(staged_content(&root, "s.rs"), "new\n");
}

#[tokio::test]
async fn stage_patch_selective_hunk() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "sel.rs", "a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");

    // Stage only the second hunk (e→E).
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "sel.rs", "ids": [1]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "sel.rs"), "a\nb\nc\nd\nE\n");
}

#[tokio::test]
async fn stage_patch_needs_input_without_answers() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "q.rs", "old\n", "new\n");

    let outcome = run_outcome(
        ctx(&root),
        tool(
            "git_stage_patch",
            &json!({"patches": [{"path": "q.rs", "ids": [0]}]}),
        ),
    )
    .await;

    assert!(
        matches!(outcome, Outcome::NeedsInput { .. }),
        "Expected NeedsInput, got {outcome:?}"
    );
}

#[tokio::test]
async fn stage_patch_declined() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "no.rs", "old\n", "new\n");

    let content = run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "no.rs", "ids": [0]}]}),
            &json!({"stage_changes": false}),
        ),
    )
    .await;

    assert_eq!(content, "Changes not staged.");
    assert_eq!(staged_content(&root, "no.rs"), "old\n");
}

// --- git_stage_patch_lines ---

#[tokio::test]
async fn stage_patch_lines_partial_hunk() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "adj.rs", "aaa\nbbb\nccc\n", "AAA\nBBB\nccc\n");

    // The hunk has 4 lines: [0]-aaa [1]-bbb [2]+AAA [3]+BBB
    // Stage only the first replacement (lines 0 and 2).
    let content = run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "adj.rs", "patch_id": 0, "lines": [0, 2]}),
        ),
    )
    .await;

    assert_eq!(content, "Patch applied.");
    assert_eq!(staged_content(&root, "adj.rs"), "AAA\nbbb\nccc\n");
}

#[tokio::test]
async fn stage_patch_lines_second_replacement() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "adj2.rs", "aaa\nbbb\nccc\n", "AAA\nBBB\nccc\n");

    // Stage only the second replacement (lines 1 and 3).
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "adj2.rs", "patch_id": 0, "lines": [1, 3]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "adj2.rs"), "aaa\nBBB\nccc\n");
}

#[tokio::test]
async fn stage_patch_lines_all_lines_same_as_full_hunk() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "all.rs", "old\n", "new\n");

    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "all.rs", "patch_id": 0, "lines": [0, 1]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "all.rs"), "new\n");
}

#[tokio::test]
async fn stage_patch_lines_pure_addition_from_intent_to_add() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("new.rs"), "line1\nline2\nline3\n").unwrap();

    run_ok(
        ctx(&root),
        tool("git_add_intent", &json!({"paths": ["new.rs"]})),
    )
    .await;

    // Stage only first two lines.
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "new.rs", "patch_id": 0, "lines": [0, 1]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "new.rs"), "line1\nline2\n");
    assert_eq!(
        fs::read_to_string(root.join("new.rs")).unwrap(),
        "line1\nline2\nline3\n"
    );
}

#[tokio::test]
async fn stage_patch_lines_out_of_range_error() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "oob.rs", "old\n", "new\n");

    let result = tools::run(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "oob.rs", "patch_id": 0, "lines": [99]}),
        ),
    )
    .await;

    assert!(result.is_err(), "Expected error for out-of-range line");
}

#[tokio::test]
async fn stage_patch_lines_empty_lines_error() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "empty.rs", "old\n", "new\n");

    let result = tools::run(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "empty.rs", "patch_id": 0, "lines": []}),
        ),
    )
    .await;

    assert!(result.is_err(), "Expected error for empty lines");
}

// --- git_diff ---

#[tokio::test]
async fn diff_shows_unstaged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "d.rs", "before\n", "after\n");

    let content = run_ok(ctx(&root), tool("git_diff", &json!({"paths": ["d.rs"]}))).await;

    assert!(content.contains("-before"));
    assert!(content.contains("+after"));
}

#[tokio::test]
async fn diff_cached_shows_staged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "dc.rs", "before\n", "after\n");

    // Stage the change.
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "dc.rs", "ids": [0]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    let content = run_ok(
        ctx(&root),
        tool("git_diff", &json!({"paths": ["dc.rs"], "cached": true})),
    )
    .await;

    assert!(content.contains("-before"));
    assert!(content.contains("+after"));
}

// --- git_unstage ---

#[tokio::test]
async fn unstage_reverts_staged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "u.rs", "old\n", "new\n");

    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "u.rs", "ids": [0]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "u.rs"), "new\n");

    let content = run_ok(ctx(&root), tool("git_unstage", &json!({"paths": ["u.rs"]}))).await;

    assert_eq!(content, "Changes unstaged.");
    assert_eq!(staged_content(&root, "u.rs"), "old\n");
}

// --- git_commit ---

#[tokio::test]
async fn commit_staged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "c.rs", "v1\n", "v2\n");

    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "c.rs", "ids": [0]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    let content = run_ok(
        ctx(&root),
        tool("git_commit", &json!({"message": "update c.rs"})),
    )
    .await;

    assert!(content.contains("update c.rs"));

    // After commit, no staged diff remains.
    let diff = git(&root, &["diff", "--cached", "--name-only"]);
    assert!(diff.trim().is_empty());
}

// --- end-to-end workflows ---

#[tokio::test]
async fn full_workflow_add_intent_list_stage_lines_commit() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();

    // New untracked file with 3 lines.
    fs::write(root.join("feature.rs"), "fn a() {}\nfn b() {}\nfn c() {}\n").unwrap();

    // Intent-to-add.
    run_ok(
        ctx(&root),
        tool("git_add_intent", &json!({"paths": ["feature.rs"]})),
    )
    .await;

    // List patches — all lines are additions.
    let patches = run_ok(
        ctx(&root),
        tool("git_list_patches", &json!({"files": ["feature.rs"]})),
    )
    .await;

    assert!(patches.contains("[0] +fn a() {}"));
    assert!(patches.contains("[1] +fn b() {}"));
    assert!(patches.contains("[2] +fn c() {}"));

    // Stage only the first two functions.
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "feature.rs", "patch_id": 0, "lines": [0, 1]}),
        ),
    )
    .await;

    assert_eq!(
        staged_content(&root, "feature.rs"),
        "fn a() {}\nfn b() {}\n"
    );

    // Commit.
    run_ok(
        ctx(&root),
        tool("git_commit", &json!({"message": "add a and b"})),
    )
    .await;

    // The third line remains unstaged in the working tree.
    let remaining = fs::read_to_string(root.join("feature.rs")).unwrap();
    assert_eq!(remaining, "fn a() {}\nfn b() {}\nfn c() {}\n");
}

#[tokio::test]
async fn stage_unstage_restage_roundtrip() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "rt.rs", "v1\n", "v2\n");

    // Stage.
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "rt.rs", "ids": [0]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "rt.rs"), "v2\n");

    // Unstage.
    run_ok(
        ctx(&root),
        tool("git_unstage", &json!({"paths": ["rt.rs"]})),
    )
    .await;

    assert_eq!(staged_content(&root, "rt.rs"), "v1\n");

    // Re-stage via stage_patch_lines this time.
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "rt.rs", "patch_id": 0, "lines": [0, 1]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "rt.rs"), "v2\n");
}

#[tokio::test]
async fn sequential_staging_across_tools() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "mix.rs", "a\nb\nc\nd\ne\n", "A\nB\nc\nd\nE\n");

    // Hunk 0: a→A, b→B (adjacent, single hunk)
    // Hunk 1: e→E
    // Stage hunk 1 fully via git_stage_patch.
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "mix.rs", "ids": [1]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "mix.rs"), "a\nb\nc\nd\nE\n");

    // Now stage only the 'a→A' part of hunk 0 via git_stage_patch_lines.
    // After staging hunk 1, the remaining diff has hunk 0 as patch_id 0.
    // [0] -a [1] -b [2] +A [3] +B — select [0, 2] for just the a→A change.
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "mix.rs", "patch_id": 0, "lines": [0, 2]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "mix.rs"), "A\nb\nc\nd\nE\n");
}

#[tokio::test]
async fn stage_patch_multiple_files_single_call() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "a.rs", "old_a\n", "new_a\n");
    commit_then_modify(&root, "b.rs", "old_b\n", "new_b\n");

    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [
                {"path": "a.rs", "ids": [0]},
                {"path": "b.rs", "ids": [0]}
            ]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "a.rs"), "new_a\n");
    assert_eq!(staged_content(&root, "b.rs"), "new_b\n");
}
