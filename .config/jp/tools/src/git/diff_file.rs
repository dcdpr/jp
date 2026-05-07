use std::fmt::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use super::{
    diff::DiffStatus,
    diff_filter::{grep_diff, truncate_diff},
};
use crate::util::{
    OneOrMany, ToolResult, error,
    runner::{DuctProcessRunner, ProcessRunner},
};

/// Maximum lines of diff output before truncation kicks in.
///
/// Matches `git_diff_commit` — both tools serve the same drill-down purpose
/// (give me the full diff for these specific files) and should behave
/// consistently from the caller's point of view.
const MAX_LINES: usize = 500;

pub(crate) async fn git_diff_file(
    root: Utf8PathBuf,
    status: String,
    paths: OneOrMany<String>,
    pattern: Option<String>,
    context: Option<usize>,
    options: &Map<String, Value>,
) -> ToolResult {
    let status = DiffStatus::parse(&status)?;
    let env = super::env_from_options(options);
    let paths = paths.iter().map(AsRef::as_ref).collect::<Vec<_>>();

    // An empty `paths` array still deserializes successfully (the schema's
    // `required` only checks presence). Without this guard the tool would run
    // git with no pathspec and dump the entire working-tree or staged diff,
    // defeating its drill-down purpose.
    if paths.is_empty() {
        return error(
            "`paths` must contain at least one entry. `git_diff_file` requires explicit paths to \
             prevent dumping the whole diff; use `git_diff` for an overview.",
        );
    }

    git_diff_file_impl(
        &root,
        status,
        &paths,
        pattern.as_deref(),
        context,
        &DuctProcessRunner,
        &env,
    )
}

fn git_diff_file_impl<R: ProcessRunner>(
    root: &Utf8Path,
    status: DiffStatus,
    paths: &[&str],
    pattern: Option<&str>,
    context: Option<usize>,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    // Same flags as `git_diff` — the only difference is that we always pass
    // explicit paths and we don't apply per-file truncation.
    let mut args: Vec<&str> = match status {
        DiffStatus::Staged => vec![
            "diff-index",
            "--cached",
            "--ita-invisible-in-index",
            "-p",
            "HEAD",
            "--",
        ],
        DiffStatus::Unstaged => vec!["diff-files", "-p", "--"],
    };
    args.extend(paths);

    let output = runner.run_with_env("git", &args, root, env)?;

    if !output.status.is_success() {
        return error(format!("git diff failed: {}", output.stderr.trim()));
    }

    let diff = output.stdout.trim_start().to_string();

    if diff.is_empty() {
        return Ok("No changes for the specified paths.".into());
    }

    let (content, note) = match pattern {
        Some(pat) => grep_diff(&diff, pat, context.unwrap_or(3))?,
        None => truncate_diff(&diff, MAX_LINES),
    };

    let mut result = String::new();
    write!(result, "```diff\n{}\n```", content.trim_end())?;
    if let Some(note) = note {
        writeln!(result, "\n\n{note}\n")?;
    }
    Ok(result.into())
}

#[cfg(test)]
#[path = "diff_file_tests.rs"]
mod tests;
