use std::path::{Path, PathBuf};

use jp_tool::{AnswerType, Context, Outcome, Question};
use serde_json::{Map, Value};

use crate::util::{
    OneOrMany, ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) async fn git_stage_patch(
    ctx: Context,
    answers: &Map<String, Value>,
    path: PathBuf,
    patch_ids: OneOrMany<usize>,
) -> ToolResult {
    git_stage_patch_impl(&ctx, answers, &path, &patch_ids, &DuctProcessRunner)
}

fn git_stage_patch_impl<R: ProcessRunner>(
    ctx: &Context,
    answers: &Map<String, Value>,
    path: &Path,
    patch_ids: &[usize],
    runner: &R,
) -> ToolResult {
    let path_str = path.to_str().unwrap_or_default();

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run("git", &["ls-files", path_str], &ctx.root)?;

    if !status.is_success() {
        return Err(format!("Failed to list staged changes: {stderr}").into());
    }

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = if stdout.is_empty() {
        // Untracked files.
        runner.run(
            "git",
            &[
                "diff",
                "--no-index",
                "--minimal",
                "--unified=0",
                "--",
                "/dev/null",
                path_str,
            ],
            &ctx.root,
        )?
    } else {
        // Tracked files.
        runner.run(
            "git",
            &[
                "diff-files",
                "-p",
                "--minimal",
                "--unified=0",
                "--",
                path_str,
            ],
            &ctx.root,
        )?
    };

    if !status.is_success() {
        return Err(format!("Failed to get patch for `{}`: {stderr}", path.display()).into());
    }

    let mut hunks = vec![];
    for (id, hunk) in stdout.split("\n@@ ").skip(1).enumerate() {
        if !patch_ids.contains(&id) {
            continue;
        }

        hunks.push(format!("@@ {hunk}"));
    }

    if hunks.is_empty() {
        return Err(format!("Failed to find patch for `{}`: {stdout}", path.display()).into());
    }

    let patch_hunks = hunks.join("\n");

    let path_display = path.display().to_string();
    let patch = indoc::formatdoc! {"
            diff --git a/{path_display} b/{path_display}
            --- a/{path_display}
            +++ b/{path_display}
            {patch_hunks}
        "};

    if ctx.action.is_format_arguments() {
        return Ok(patch.into());
    }

    match answers.get("stage_changes").and_then(Value::as_bool) {
        Some(true) => {}
        Some(false) => {
            return Ok("Changes not staged.".into());
        }
        None => {
            return Ok(Outcome::NeedsInput {
                question: Question {
                    id: "stage_changes".to_string(),
                    text: format!("Do you want to stage the following patch?\n\n{patch}"),
                    answer_type: AnswerType::Boolean,
                    default: Some(Value::Bool(true)),
                },
            });
        }
    }

    let ProcessOutput { stderr, status, .. } = runner.run_with_env_and_stdin(
        "git",
        &["apply", "--cached", "--unidiff-zero", "-"],
        &ctx.root,
        &[],
        Some(&patch),
    )?;

    if !status.is_success() {
        return Err(format!("Failed to apply patch: {stderr}").into());
    }

    Ok("Patch applied.".into())
}

#[cfg(test)]
mod tests {
    use camino_tempfile::tempdir;
    use jp_tool::Action;
    use serde_json::json;

    use super::*;
    use crate::util::runner::MockProcessRunner;

    #[test]
    fn test_git_stage_patch_success() {
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

        let result = git_stage_patch_impl(
            &ctx,
            &answers,
            &std::path::PathBuf::from("test.rs"),
            &[0],
            &runner,
        )
        .unwrap();

        let content = result.into_content().unwrap();
        assert_eq!(content, "Patch applied.");
    }
}
