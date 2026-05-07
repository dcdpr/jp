use std::fmt::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use super::diff_filter::{grep_diff, truncate_diff};
use crate::util::{
    OneOrMany, ToolResult, error,
    runner::{DuctProcessRunner, ProcessRunner},
};

/// Maximum lines of diff output before truncation kicks in.
const MAX_LINES: usize = 500;

pub(crate) async fn git_diff_commit(
    root: Utf8PathBuf,
    revision: String,
    paths: OneOrMany<String>,
    pattern: Option<String>,
    context: Option<usize>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = super::env_from_options(options);
    let paths = paths.iter().map(AsRef::as_ref).collect::<Vec<_>>();

    git_diff_commit_impl(
        &root,
        &revision,
        &paths,
        pattern.as_deref(),
        context,
        &DuctProcessRunner,
        &env,
    )
}

fn git_diff_commit_impl<R: ProcessRunner>(
    root: &Utf8Path,
    revision: &str,
    paths: &[&str],
    pattern: Option<&str>,
    context: Option<usize>,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    // `git show <rev> --format= -- <paths>` gives us just the diff for
    // specific files, with an empty format to suppress the commit header.
    let mut args: Vec<&str> = vec!["show", "--format=", revision, "--"];
    args.extend(paths);

    let output = runner.run_with_env("git", &args, root, env)?;

    if !output.status.is_success() {
        return error(format!("git show failed: {}", output.stderr.trim()));
    }

    let diff = output.stdout.trim_start().to_string();

    if diff.is_empty() {
        return Ok("No diff found for the specified revision and paths.".into());
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
#[path = "diff_commit_tests.rs"]
mod tests;
