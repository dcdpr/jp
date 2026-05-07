use std::fmt::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use super::diff_filter::{
    add_slice_markers, grep_diff, slice_diff, truncate_diff, validate_line_range,
};
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
    start_line: Option<usize>,
    end_line: Option<usize>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = super::env_from_options(options);
    let paths = paths.iter().map(AsRef::as_ref).collect::<Vec<_>>();

    if let Err(msg) = validate_line_range(start_line, end_line) {
        return error(msg);
    }

    git_diff_commit_impl(
        &root,
        &revision,
        &paths,
        pattern.as_deref(),
        context,
        start_line,
        end_line,
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
    start_line: Option<usize>,
    end_line: Option<usize>,
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

    let total_lines = diff.lines().count();
    if let Some(s) = start_line
        && s > total_lines
    {
        return error(format!(
            "`start_line` is greater than the number of diff output lines ({total_lines})."
        ));
    }

    let has_range = start_line.is_some() || end_line.is_some();

    // Apply slice first if a range was requested. An explicit range bypasses
    // the truncation cap — the user is paginating and owns their window size.
    let working = if has_range {
        slice_diff(&diff, start_line, end_line)
    } else {
        diff
    };

    // Then either grep (slice-then-grep), pass through (range-only), or
    // fall back to the default truncation cap.
    let (mut content, note): (String, Option<String>) = if let Some(pat) = pattern {
        let (c, n) = grep_diff(&working, pat, context.unwrap_or(3))?;
        (c.into_owned(), n)
    } else if has_range {
        (working, None)
    } else {
        let (c, n) = truncate_diff(&working, MAX_LINES);
        (c.into_owned(), n)
    };

    // Slice markers are added last so they survive grep filtering.
    if has_range {
        add_slice_markers(&mut content, start_line, end_line);
    }

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
