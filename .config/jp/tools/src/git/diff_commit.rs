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

    // An empty `paths` array still deserializes successfully (the schema's
    // `required` only checks presence). Without this guard the tool would
    // run `git show <rev> --` with no pathspec and dump the entire commit
    // diff, defeating the drill-down purpose of the `paths` argument.
    if paths.is_empty() {
        return error(
            "`paths` must contain at least one entry. `git_diff_commit` requires explicit paths \
             to prevent dumping the whole commit diff; use `git_show` for an overview.",
        );
    }

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

    // An explicit range bypasses the truncation cap — the user is paginating
    // and owns their window size. Three modes:
    //
    // - `pattern` (with or without `range`): grep walks the full diff so
    //   structural headers and `@@` line counters stay accurate. When a
    //   range is also set, `grep_diff` restricts matches to that window
    //   instead of pre-slicing, which would hide preceding `@@` headers and
    //   produce zero-based synthesized hunk headers.
    // - `range` only: a plain text slice of the rendered diff.
    // - neither: fall back to the default truncation cap.
    let (mut content, note): (String, Option<String>) = if let Some(pat) = pattern {
        let bounds = has_range.then(|| (start_line.unwrap_or(1), end_line.unwrap_or(total_lines)));
        let (c, n) = grep_diff(&diff, pat, context.unwrap_or(3), bounds)?;
        (c.into_owned(), n)
    } else if has_range {
        (slice_diff(&diff, start_line, end_line), None)
    } else {
        let (c, n) = truncate_diff(&diff, MAX_LINES);
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
