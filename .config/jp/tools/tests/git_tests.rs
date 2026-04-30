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

/// Build the shared tool options that isolate git from host config.
///
/// Without this, tools that spawn git via `DuctProcessRunner` would inherit
/// the developer's (or CI runner's) global git configuration, which can cause
/// flaky failures (e.g. `commit.gpgsign = true`, global hooks, etc.).
fn git_options() -> Map<String, Value> {
    json!({
        "env": {
            "GIT_CONFIG_GLOBAL": "",
            "GIT_CONFIG_SYSTEM": ""
        }
    })
    .as_object()
    .unwrap()
    .clone()
}

fn tool(name: &str, arguments: &Value) -> Tool {
    Tool {
        name: name.to_string(),
        arguments: arguments.as_object().unwrap().clone(),
        answers: Map::new(),
        options: git_options(),
    }
}

fn tool_with_answers(name: &str, arguments: &Value, answers: &Value) -> Tool {
    Tool {
        name: name.to_string(),
        arguments: arguments.as_object().unwrap().clone(),
        answers: answers.as_object().unwrap().clone(),
        options: git_options(),
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

/// 20-line fixture used by the offset-correction regression tests.
fn lines_1_to_20() -> String {
    let mut s = String::new();
    for i in 1..=20 {
        use std::fmt::Write as _;
        writeln!(s, "line{i}").unwrap();
    }
    s
}

/// Extract content-addressed patch IDs from a `git_list_patches` XML response.
fn extract_patch_ids(xml: &str) -> Vec<String> {
    xml.lines()
        .filter_map(|line| {
            let l = line.trim();
            l.strip_prefix("<id>")
                .and_then(|s| s.strip_suffix("</id>"))
                .map(str::to_string)
        })
        .collect()
}

/// Fetch the content-addressed patch IDs for a single file in file order.
///
/// Tests that need to stage a hunk by position pull the IDs through this
/// helper, since the staging tools no longer accept positional indices.
async fn patch_ids(root: &Utf8Path, path: &str) -> Vec<String> {
    let raw = run_ok(
        ctx(root),
        tool("git_list_patches", &json!({"files": [path]})),
    )
    .await;
    extract_patch_ids(&raw)
}

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

#[tokio::test]
async fn list_patches_without_files_discovers_all_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "a.rs", "old_a\n", "new_a\n");
    commit_then_modify(&root, "b.rs", "old_b\n", "new_b\n");

    // No `files` argument — should discover both changed files.
    let content = run_ok(ctx(&root), tool("git_list_patches", &json!({}))).await;

    assert!(content.contains("<path>a.rs</path>"));
    assert!(content.contains("<path>b.rs</path>"));
    assert!(content.contains("-old_a"));
    assert!(content.contains("+new_a"));
    assert!(content.contains("-old_b"));
    assert!(content.contains("+new_b"));
}

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
    assert_eq!(extract_patch_ids(&content).len(), 1);
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

    let ids = extract_patch_ids(&content);
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
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
    let ids = patch_ids(&root, "bye.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "bye.rs", "ids": [&ids[0]]}]}),
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

    let ids = patch_ids(&root, "s.rs").await;
    let content = run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "s.rs", "ids": [&ids[0]]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(content, "Patch applied.");
    assert_eq!(staged_content(&root, "s.rs"), "new\n");
}

#[tokio::test]
async fn stage_patch_non_last_hunk() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "nl.rs", "a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");

    // Stage only the FIRST hunk (a→A), which is not the last.
    let ids = patch_ids(&root, "nl.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "nl.rs", "ids": [&ids[0]]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "nl.rs"), "A\nb\nc\nd\ne\n");
}

#[tokio::test]
async fn stage_patch_selective_hunk() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "sel.rs", "a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");

    // Stage only the second hunk (e→E).
    let ids = patch_ids(&root, "sel.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "sel.rs", "ids": [&ids[1]]}]}),
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

    let ids = patch_ids(&root, "q.rs").await;
    let outcome = run_outcome(
        ctx(&root),
        tool(
            "git_stage_patch",
            &json!({"patches": [{"path": "q.rs", "ids": [&ids[0]]}]}),
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

    let ids = patch_ids(&root, "no.rs").await;
    let content = run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "no.rs", "ids": [&ids[0]]}]}),
            &json!({"stage_changes": false}),
        ),
    )
    .await;

    assert_eq!(content, "Changes not staged.");
    assert_eq!(staged_content(&root, "no.rs"), "old\n");
}

#[tokio::test]
async fn stage_patch_lines_partial_hunk() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "adj.rs", "aaa\nbbb\nccc\n", "AAA\nBBB\nccc\n");

    // The hunk has 4 lines: [0]-aaa [1]-bbb [2]+AAA [3]+BBB
    // Stage only the first replacement (lines 0 and 2).
    let ids = patch_ids(&root, "adj.rs").await;
    let content = run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "adj.rs", "patch_id": &ids[0], "lines": [0, 2]}),
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
    let ids = patch_ids(&root, "adj2.rs").await;
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "adj2.rs", "patch_id": &ids[0], "lines": [1, 3]}),
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

    let ids = patch_ids(&root, "all.rs").await;
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "all.rs", "patch_id": &ids[0], "lines": [0, 1]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "all.rs"), "new\n");
}

#[tokio::test]
async fn stage_patch_lines_pure_insertion_into_tracked_file() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    // Insert a new line between existing lines.
    commit_then_modify(&root, "ins.rs", "aaa\nbbb\nccc\n", "aaa\nbbb\nNEW\nccc\n");

    // The diff inserts NEW after line 2: @@ -2,0 +3,1 @@
    let ids = patch_ids(&root, "ins.rs").await;
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "ins.rs", "patch_id": &ids[0], "lines": [0]}),
        ),
    )
    .await;

    // The insertion must land AFTER "bbb", not before it.
    assert_eq!(staged_content(&root, "ins.rs"), "aaa\nbbb\nNEW\nccc\n");
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
    let ids = patch_ids(&root, "new.rs").await;
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "new.rs", "patch_id": &ids[0], "lines": [0, 1]}),
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
async fn stage_patch_lines_with_range() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "rng.rs", "aaa\nbbb\nccc\n", "AAA\nBBB\nccc\n");

    // Hunk: [0]-aaa [1]-bbb [2]+AAA [3]+BBB
    // Stage all four lines via a single range.
    let ids = patch_ids(&root, "rng.rs").await;
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "rng.rs", "patch_id": &ids[0], "lines": ["0:3"]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "rng.rs"), "AAA\nBBB\nccc\n");
}

#[tokio::test]
async fn stage_patch_lines_mixed_integers_and_ranges() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "mix2.rs", "aaa\nbbb\nccc\n", "AAA\nBBB\nccc\n");

    // Hunk: [0]-aaa [1]-bbb [2]+AAA [3]+BBB
    // Stage only the first replacement using mixed format: integer 0, range "2:2".
    let ids = patch_ids(&root, "mix2.rs").await;
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "mix2.rs", "patch_id": &ids[0], "lines": [0, "2:2"]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "mix2.rs"), "AAA\nbbb\nccc\n");
}

#[tokio::test]
async fn stage_patch_lines_out_of_range_error() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "oob.rs", "old\n", "new\n");

    let ids = patch_ids(&root, "oob.rs").await;
    let result = tools::run(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "oob.rs", "patch_id": &ids[0], "lines": [99]}),
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

    let ids = patch_ids(&root, "empty.rs").await;
    let result = tools::run(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "empty.rs", "patch_id": &ids[0], "lines": []}),
        ),
    )
    .await;

    assert!(result.is_err(), "Expected error for empty lines");
}

#[tokio::test]
async fn diff_shows_unstaged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "d.rs", "before\n", "after\n");

    let content = run_ok(
        ctx(&root),
        tool(
            "git_diff",
            &json!({"paths": ["d.rs"], "status": "unstaged"}),
        ),
    )
    .await;

    assert!(content.contains("-before"));
    assert!(content.contains("+after"));
}

#[tokio::test]
async fn diff_without_paths_diffs_entire_repo() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "x.rs", "old_x\n", "new_x\n");
    commit_then_modify(&root, "y.rs", "old_y\n", "new_y\n");

    let content = run_ok(ctx(&root), tool("git_diff", &json!({"status": "unstaged"}))).await;

    assert!(content.contains("-old_x"));
    assert!(content.contains("+new_x"));
    assert!(content.contains("-old_y"));
    assert!(content.contains("+new_y"));
}

#[tokio::test]
async fn diff_cached_shows_staged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "dc.rs", "before\n", "after\n");

    // Stage the change.
    let ids = patch_ids(&root, "dc.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "dc.rs", "ids": [&ids[0]]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    let content = run_ok(
        ctx(&root),
        tool("git_diff", &json!({"paths": ["dc.rs"], "status": "staged"})),
    )
    .await;

    assert!(content.contains("-before"));
    assert!(content.contains("+after"));
}

#[tokio::test]
async fn diff_unstaged_excludes_staged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "sep.rs", "before\n", "after\n");

    // Stage the change.
    let ids = patch_ids(&root, "sep.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "sep.rs", "ids": [&ids[0]]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    // Unstaged diff must be empty — the change is fully staged.
    let unstaged = run_ok(
        ctx(&root),
        tool(
            "git_diff",
            &json!({"paths": ["sep.rs"], "status": "unstaged"}),
        ),
    )
    .await;
    assert!(
        !unstaged.contains("-before") && !unstaged.contains("+after"),
        "unstaged diff should not contain staged changes, got: {unstaged}"
    );
}

#[tokio::test]
async fn unstage_reverts_staged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "u.rs", "old\n", "new\n");

    let ids = patch_ids(&root, "u.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "u.rs", "ids": [&ids[0]]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "u.rs"), "new\n");

    let content = run_ok(ctx(&root), tool("git_unstage", &json!({"paths": ["u.rs"]}))).await;

    assert_eq!(content, "Changes unstaged.");
    assert_eq!(staged_content(&root, "u.rs"), "old\n");
}

#[tokio::test]
async fn commit_staged_changes() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "c.rs", "v1\n", "v2\n");

    let ids = patch_ids(&root, "c.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "c.rs", "ids": [&ids[0]]}]}),
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
    let ids = extract_patch_ids(&patches);
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "feature.rs", "patch_id": &ids[0], "lines": [0, 1]}),
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
    let ids = patch_ids(&root, "rt.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "rt.rs", "ids": [&ids[0]]}]}),
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

    // Re-stage via stage_patch_lines this time. Re-list because the index
    // changed; the new ID is content-addressed and stable across this
    // round-trip, but we still resolve it through the listing.
    let ids = patch_ids(&root, "rt.rs").await;
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "rt.rs", "patch_id": &ids[0], "lines": [0, 1]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "rt.rs"), "v2\n");
}

#[tokio::test]
async fn log_lists_recent_commits() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "a.rs", "v1\n", "v2\n");
    git(&root, &["add", "a.rs"]);
    git(&root, &["commit", "-m", "update a.rs"]);

    let content = run_ok(ctx(&root), tool("git_log", &json!({}))).await;

    assert!(content.contains("update a.rs"));
    assert!(content.contains("init"));
    assert!(content.contains("short_hash: "));
}

#[tokio::test]
async fn log_filters_by_query() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "b.rs", "v1\n", "v2\n");
    git(&root, &["add", "b.rs"]);
    git(&root, &["commit", "-m", "feat: add widget"]);

    commit_then_modify(&root, "c.rs", "v1\n", "v2\n");
    git(&root, &["add", "c.rs"]);
    git(&root, &["commit", "-m", "fix: correct typo"]);

    let content = run_ok(ctx(&root), tool("git_log", &json!({"query": "widget"}))).await;

    assert!(content.contains("widget"));
    assert!(!content.contains("typo"));
}

#[tokio::test]
async fn log_filters_by_path() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "x.rs", "v1\n", "v2\n");
    git(&root, &["add", "x.rs"]);
    git(&root, &["commit", "-m", "change x"]);

    commit_then_modify(&root, "y.rs", "v1\n", "v2\n");
    git(&root, &["add", "y.rs"]);
    git(&root, &["commit", "-m", "change y"]);

    let content = run_ok(ctx(&root), tool("git_log", &json!({"paths": ["x.rs"]}))).await;

    assert!(content.contains("change x"));
    assert!(!content.contains("change y"));
}

#[tokio::test]
async fn log_respects_count() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    for i in 0..5 {
        let path = format!("{i}.rs");
        fs::write(root.join(&path), format!("v{i}\n")).unwrap();
        git(&root, &["add", &path]);
        git(&root, &["commit", "-m", &format!("commit {i}")]);
    }

    let content = run_ok(ctx(&root), tool("git_log", &json!({"count": 2}))).await;

    // Should have exactly 2 log entries.
    assert!(content.contains("commit 4"));
    assert!(content.contains("commit 3"));
    assert!(!content.contains("commit 2"));
}

#[tokio::test]
async fn log_empty_result() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();

    let content = run_ok(
        ctx(&root),
        tool("git_log", &json!({"query": "nonexistent_query_string_xyz"})),
    )
    .await;

    assert!(content.contains("No commits found"));
}

#[tokio::test]
async fn show_displays_commit_details() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "s.rs", "old\n", "new\n");
    git(&root, &["add", "s.rs"]);
    git(&root, &[
        "commit",
        "-m",
        "feat: update s.rs\n\nDetailed description.",
    ]);

    // Get the latest commit hash.
    let hash = git(&root, &["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();

    let content = run_ok(ctx(&root), tool("git_show", &json!({"revision": "HEAD"}))).await;

    assert!(content.contains(&hash));
    assert!(content.contains("feat: update s.rs"));
    assert!(content.contains("Detailed description."));
    assert!(content.contains("    - s.rs ("));
    assert!(content.contains("  <files>"));
}

#[tokio::test]
async fn show_bad_revision_errors() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();

    let outcome = run_outcome(
        ctx(&root),
        tool("git_show", &json!({"revision": "nonexistent_ref_xyz"})),
    )
    .await;

    assert!(
        matches!(outcome, Outcome::Error { .. }),
        "Expected error for bad revision, got {outcome:?}"
    );
}

#[tokio::test]
async fn diff_commit_shows_file_diff() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "d.rs", "old\n", "new\n");
    git(&root, &["add", "d.rs"]);
    git(&root, &["commit", "-m", "change d"]);

    let content = run_ok(
        ctx(&root),
        tool(
            "git_diff_commit",
            &json!({"revision": "HEAD", "paths": ["d.rs"]}),
        ),
    )
    .await;

    assert!(content.contains("-old"));
    assert!(content.contains("+new"));
}

#[tokio::test]
async fn diff_commit_excludes_unrequested_files() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    fs::write(root.join("a.rs"), "a_old\n").unwrap();
    fs::write(root.join("b.rs"), "b_old\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "add both"]);

    fs::write(root.join("a.rs"), "a_new\n").unwrap();
    fs::write(root.join("b.rs"), "b_new\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "change both"]);

    // Only request diff for a.rs.
    let content = run_ok(
        ctx(&root),
        tool(
            "git_diff_commit",
            &json!({"revision": "HEAD", "paths": ["a.rs"]}),
        ),
    )
    .await;

    assert!(content.contains("a_new") || content.contains("a_old"));
    assert!(!content.contains("b_new") && !content.contains("b_old"));
}

#[tokio::test]
async fn diff_commit_with_pattern() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    // Create a file with enough content to have interesting grep results.
    let original = (0..20)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let modified = original
        .replace("line 5", "CHANGED_5")
        .replace("line 15", "CHANGED_15");

    fs::write(root.join("g.rs"), &original).unwrap();
    git(&root, &["add", "g.rs"]);
    git(&root, &["commit", "-m", "add g.rs"]);

    fs::write(root.join("g.rs"), &modified).unwrap();
    git(&root, &["add", "g.rs"]);
    git(&root, &["commit", "-m", "modify g.rs"]);

    let content = run_ok(
        ctx(&root),
        tool(
            "git_diff_commit",
            &json!({"revision": "HEAD", "paths": ["g.rs"], "pattern": "CHANGED_5", "context": 1}),
        ),
    )
    .await;

    assert!(content.contains("CHANGED_5"));
    // With context=1, CHANGED_15 should not appear (they're far apart).
    assert!(!content.contains("CHANGED_15"));
}

#[tokio::test]
async fn diff_commit_no_match_for_path() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "exists.rs", "old\n", "new\n");
    git(&root, &["add", "exists.rs"]);
    git(&root, &["commit", "-m", "change"]);

    let content = run_ok(
        ctx(&root),
        tool(
            "git_diff_commit",
            &json!({"revision": "HEAD", "paths": ["nonexistent.rs"]}),
        ),
    )
    .await;

    assert!(content.contains("No diff found"));
}

#[tokio::test]
async fn staged_diff_excludes_intent_to_add_files() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();

    // Also stage a real change so we can verify the diff isn't just empty.
    commit_then_modify(&root, "tracked.rs", "old\n", "new\n");
    git(&root, &["add", "tracked.rs"]);

    // Create an untracked file and mark it intent-to-add.
    fs::write(root.join("ita.rs"), "intent to add content\n").unwrap();
    git(&root, &["add", "--intent-to-add", "ita.rs"]);

    let content = run_ok(ctx(&root), tool("git_diff", &json!({"status": "staged"}))).await;

    // The real staged change must be present.
    assert!(
        content.contains("tracked.rs"),
        "staged diff should contain the genuinely staged file"
    );

    // The intent-to-add file must NOT appear in staged output.
    assert!(
        !content.contains("ita.rs"),
        "staged diff must not contain intent-to-add file, got: {content}"
    );
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
    // Stage hunk 1 fully via git_stage_patch. Capture hunk 0's ID up
    // front: with content-addressed IDs it stays valid after staging
    // hunk 1, which is part of the point.
    let ids = patch_ids(&root, "mix.rs").await;
    let hunk_0_id = ids[0].clone();
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "mix.rs", "ids": [&ids[1]]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "mix.rs"), "a\nb\nc\nd\nE\n");

    // Now stage only the 'a→A' part of hunk 0 via git_stage_patch_lines.
    // The pre-staging ID is still valid because IDs are content-addressed
    // and hunk 0 itself was untouched by the previous operation.
    // [0] -a [1] -b [2] +A [3] +B — select [0, 2] for just the a→A change.
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "mix.rs", "patch_id": &hunk_0_id, "lines": [0, 2]}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "mix.rs"), "A\nb\nc\nd\nE\n");
}

#[tokio::test]
async fn stage_patch_addition_with_unrelated_unstaged_removal() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    let original = lines_1_to_20();
    let modified: String = {
        let mut lines: Vec<String> = original.lines().map(String::from).collect();
        for _ in 0..9 {
            lines.remove(2);
        }
        lines.insert(9, "INSERTED".to_string());
        lines.join("\n") + "\n"
    };

    fs::write(root.join("sp.rs"), &original).unwrap();
    git(&root, &["add", "sp.rs"]);
    git(&root, &["commit", "-m", "init sp"]);
    fs::write(root.join("sp.rs"), &modified).unwrap();

    let ids = patch_ids(&root, "sp.rs").await;
    assert_eq!(ids.len(), 2, "expected 2 hunks, got {}", ids.len());

    // Stage only the addition hunk (the second one).
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [{"path": "sp.rs", "ids": [&ids[1]]}]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    let expected: String = {
        let mut lines: Vec<String> = original.lines().map(String::from).collect();
        lines.insert(18, "INSERTED".to_string());
        lines.join("\n") + "\n"
    };
    assert_eq!(staged_content(&root, "sp.rs"), expected);
}

#[tokio::test]
async fn stage_patch_lines_addition_with_unrelated_unstaged_removal() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();

    // Original file: 20 lines.
    let original = lines_1_to_20();

    // Working tree: remove lines 3-11 (9 lines) AND insert "INSERTED" after
    // old line 18. Two distant unstaged hunks in a single file.
    let modified: String = {
        let mut lines: Vec<String> = original.lines().map(String::from).collect();
        for _ in 0..9 {
            lines.remove(2);
        }
        // After removals: indices 0..=10 hold line1, line2, line12..line20.
        // Old line 18 is now at index 8. Insert "INSERTED" right after it.
        lines.insert(9, "INSERTED".to_string());
        lines.join("\n") + "\n"
    };

    fs::write(root.join("two_hunk.rs"), &original).unwrap();
    git(&root, &["add", "two_hunk.rs"]);
    git(&root, &["commit", "-m", "init two_hunk"]);
    fs::write(root.join("two_hunk.rs"), &modified).unwrap();

    // Expect two distinct hunks in the listing.
    let ids = patch_ids(&root, "two_hunk.rs").await;
    assert_eq!(ids.len(), 2, "expected 2 hunks, got {}", ids.len());

    // Stage just the insertion hunk's only line.
    run_ok(
        ctx(&root),
        tool(
            "git_stage_patch_lines",
            &json!({"path": "two_hunk.rs", "patch_id": &ids[1], "lines": [0]}),
        ),
    )
    .await;

    // The index should match HEAD plus exactly one inserted line after
    // old line 18 — the unrelated removal must remain unstaged.
    let expected: String = {
        let mut lines: Vec<String> = original.lines().map(String::from).collect();
        lines.insert(18, "INSERTED".to_string());
        lines.join("\n") + "\n"
    };
    assert_eq!(staged_content(&root, "two_hunk.rs"), expected);
}

#[tokio::test]
async fn stage_patch_multiple_files_single_call() {
    if !has_git() {
        return;
    }

    let (_dir, root) = init_repo();
    commit_then_modify(&root, "a.rs", "old_a\n", "new_a\n");
    commit_then_modify(&root, "b.rs", "old_b\n", "new_b\n");

    let a_ids = patch_ids(&root, "a.rs").await;
    let b_ids = patch_ids(&root, "b.rs").await;
    run_ok(
        ctx(&root),
        tool_with_answers(
            "git_stage_patch",
            &json!({"patches": [
                {"path": "a.rs", "ids": [&a_ids[0]]},
                {"path": "b.rs", "ids": [&b_ids[0]]}
            ]}),
            &json!({"stage_changes": true}),
        ),
    )
    .await;

    assert_eq!(staged_content(&root, "a.rs"), "new_a\n");
    assert_eq!(staged_content(&root, "b.rs"), "new_b\n");
}
