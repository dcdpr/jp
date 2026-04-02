use jp_tool::{AnswerType, Context, Outcome, Question};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::apply::apply_patch_to_index;
use crate::util::{
    OneOrMany, ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

#[derive(Debug, Deserialize)]
pub struct PatchTarget {
    path: String,
    ids: OneOrMany<usize>,
}

pub(crate) async fn git_stage_patch(
    ctx: Context,
    answers: &Map<String, Value>,
    patches: OneOrMany<PatchTarget>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = super::env_from_options(options);
    git_stage_patch_impl(&ctx, answers, &patches, &DuctProcessRunner, &env)
}

fn git_stage_patch_impl<R: ProcessRunner>(
    ctx: &Context,
    answers: &Map<String, Value>,
    patches: &[PatchTarget],
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    // Build patches for all targets, collecting errors per file.
    let mut built: Vec<(&str, String)> = vec![];
    let mut errors: Vec<String> = vec![];

    for target in patches {
        match build_file_patch(ctx, &target.path, &target.ids, runner, env) {
            Ok(patch) => built.push((&target.path, patch)),
            Err(error) => errors.push(format!("{}: {error}", target.path)),
        }
    }

    if built.is_empty() {
        return Err(format!("Failed to build patches:\n{}", errors.join("\n")).into());
    }

    // For `format_arguments`, show what would be applied.
    let combined: String = built
        .iter()
        .map(|(_, p)| p.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    if ctx.action.is_format_arguments() {
        return Ok(combined.into());
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
                    text: format!("Do you want to stage the following patch?\n\n{combined}"),
                    answer_type: AnswerType::Boolean,
                    default: Some(Value::Bool(true)),
                },
            });
        }
    }

    // Apply each file's patch individually so partial success is possible.
    let mut staged: Vec<&str> = vec![];

    for (path, patch) in &built {
        match apply_patch_to_index(patch, &ctx.root, runner, env) {
            Ok(()) => staged.push(path),
            Err(error) => errors.push(format!("{path}: {error}")),
        }
    }

    if errors.is_empty() {
        return Ok("Patch applied.".into());
    }

    let mut msg = String::new();
    if !staged.is_empty() {
        msg.push_str(&format!("Staged: {}\n", staged.join(", ")));
    }
    msg.push_str(&format!("Failed:\n{}", errors.join("\n")));

    if staged.is_empty() {
        Err(msg.into())
    } else {
        Ok(msg.into())
    }
}

fn build_file_patch<R: ProcessRunner>(
    ctx: &Context,
    path: &str,
    patch_ids: &[usize],
    runner: &R,
    env: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run_with_env("git", &["ls-files", path], &ctx.root, env)?;

    if !status.is_success() {
        return Err(format!("Failed to check tracking status: {stderr}").into());
    }

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = if stdout.is_empty() {
        runner.run_with_env(
            "git",
            &[
                "diff",
                "--no-index",
                "--minimal",
                "--unified=0",
                "--",
                "/dev/null",
                path,
            ],
            &ctx.root,
            env,
        )?
    } else {
        runner.run_with_env(
            "git",
            &["diff-files", "-p", "--minimal", "--unified=0", "--", path],
            &ctx.root,
            env,
        )?
    };

    if !status.is_success() {
        return Err(format!("Failed to get diff: {stderr}").into());
    }

    // Preserve the original diff header (everything before the first hunk).
    // This keeps special headers like `deleted file mode` and `+++ /dev/null`
    // that git needs to correctly stage deletions, renames, etc.
    let Some((header, _)) = stdout.split_once("\n@@ ") else {
        return Err("No hunks found".into());
    };

    let mut hunks = vec![];
    for (id, hunk) in stdout.split("\n@@ ").skip(1).enumerate() {
        if !patch_ids.contains(&id) {
            continue;
        }

        hunks.push(format!("@@ {hunk}"));
    }

    if hunks.is_empty() {
        let available: Vec<_> = (0..stdout.split("\n@@ ").skip(1).count()).collect();
        return Err(format!("Patch IDs {patch_ids:?} not found (available: {available:?})").into());
    }

    // Ensure the patch ends with a newline. Non-last hunks lose their
    // trailing newline during the `\n@@ ` split (the newline becomes part
    // of the delimiter). `git apply` requires every diff line to be
    // newline-terminated; without it the patch is rejected as corrupt.
    let mut patch = format!("{header}\n{}", hunks.join("\n"));
    if !patch.ends_with('\n') {
        patch.push('\n');
    }
    Ok(patch)
}

#[cfg(test)]
#[path = "stage_patch_tests.rs"]
mod tests;
