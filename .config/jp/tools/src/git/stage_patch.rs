use std::collections::HashMap;

use jp_tool::{AnswerType, Context, Outcome, Question};
use serde::Deserialize;
use serde_json::{Map, Value};

use super::{
    apply::apply_patch_to_index,
    hunk::{diff_header, hunk_id, rewrite_hunk_y, split_hunks},
};
use crate::util::{
    OneOrMany, ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

#[derive(Debug, Deserialize)]
pub struct PatchTarget {
    path: String,
    ids: OneOrMany<String>,
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
    requested_ids: &[String],
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
    let header = diff_header(&stdout).ok_or("No hunks found")?;

    // Index hunks by content-addressed ID. Iterating the available hunks
    // in file order preserves the order in the assembled patch, which
    // `git apply` requires for multi-hunk patches.
    let available_hunks = split_hunks(&stdout);
    let id_to_hunk: HashMap<String, &str> = available_hunks
        .iter()
        .map(|h| (hunk_id(h), h.as_str()))
        .collect();

    let mut missing: Vec<&str> = vec![];
    for id in requested_ids {
        if !id_to_hunk.contains_key(id) {
            missing.push(id.as_str());
        }
    }

    if !missing.is_empty() {
        // Rebuild available IDs in file order for a stable error message.
        let available_ids: Vec<String> = available_hunks.iter().map(|h| hunk_id(h)).collect();
        return Err(format!(
            "Patch IDs not found in current diff: {missing:?}. They may already be staged, or the \
             working tree changed since `git_list_patches`. Re-run `git_list_patches` and try \
             again. Available IDs: {available_ids:?}"
        )
        .into());
    }

    // Deduplicate requested IDs and emit hunks in file order to satisfy
    // `git apply`'s monotonic-line requirement for multi-hunk patches.
    //
    // Each selected hunk's `+Y` is recomputed: the working-tree diff bakes
    // in the cumulative line shift of every preceding unstaged hunk, but a
    // partial stage skips some of those shifts. We track the net effect of
    // preceding *selected* hunks and rewrite each header accordingly so
    // `git apply --cached --unidiff-zero` lands the change at the right
    // line.
    let requested: std::collections::HashSet<&str> =
        requested_ids.iter().map(String::as_str).collect();

    let mut cumulative_offset: isize = 0;
    let mut selected: Vec<String> = vec![];
    for hunk in &available_hunks {
        if !requested.contains(hunk_id(hunk).as_str()) {
            continue;
        }
        let (rewritten, counts) = rewrite_hunk_y(hunk, cumulative_offset)
            .ok_or_else(|| format!("Malformed hunk header in diff: {hunk}"))?;
        #[allow(clippy::cast_possible_wrap)]
        {
            cumulative_offset += counts.new_count as isize - counts.old_count as isize;
        }
        selected.push(rewritten);
    }

    // Ensure the patch ends with a newline. Hunks emitted by `split_hunks`
    // may have lost their trailing newline during splitting (the newline
    // becomes part of the `\n@@ ` delimiter). `git apply` requires every
    // diff line to be newline-terminated; without it the patch is rejected
    // as corrupt.
    let mut patch = format!("{header}\n{}", selected.join("\n"));
    if !patch.ends_with('\n') {
        patch.push('\n');
    }
    Ok(patch)
}

#[cfg(test)]
#[path = "stage_patch_tests.rs"]
mod tests;
