use std::fmt::Write;

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

use crate::util::{
    OneOrMany, ToolResult,
    runner::{DuctProcessRunner, ProcessRunner},
};

/// Maximum number of diff lines to show per file before truncation.
const MAX_LINES_PER_FILE: usize = 50;

/// Which changes to include in the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffStatus {
    /// Staged changes only (HEAD vs index).
    Staged,
    /// Unstaged changes only (index vs working tree).
    Unstaged,
}

impl DiffStatus {
    fn parse(s: &str) -> Result<Self, String> {
        match s {
            "staged" => Ok(Self::Staged),
            "unstaged" => Ok(Self::Unstaged),
            other => Err(format!(
                "Invalid diff status '{other}', expected 'staged' or 'unstaged'"
            )),
        }
    }
}

/// A single file's section from a unified diff.
struct FileDiff {
    path: String,
    text: String,
    line_count: usize,
}

fn git_diff_impl<R: ProcessRunner>(
    root: &Utf8Path,
    paths: &[String],
    status: DiffStatus,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    let mut args = match status {
        DiffStatus::Staged => vec![
            "diff-index",
            "--cached",
            "--ita-invisible-in-index",
            "-p",
            "HEAD",
        ],
        DiffStatus::Unstaged => vec!["diff-files", "-p"],
    };

    let path_refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    args.extend(path_refs);

    let output = runner.run_with_env("git", &args, root, env)?;

    let diff = output.stdout.trim();
    if diff.is_empty() {
        return Ok("No changes.".into());
    }

    Ok(format_diff(diff, MAX_LINES_PER_FILE).into())
}

/// Format a unified diff with per-file truncation.
///
/// Each file's diff is capped at `max_lines_per_file`. Truncated files get a
/// note directing the user to narrow their query with the `paths` parameter.
fn format_diff(diff: &str, max_lines_per_file: usize) -> String {
    let files = split_into_files(diff);

    if files.is_empty() {
        return "No changes.".to_string();
    }

    let mut result = String::new();
    let mut any_truncated = false;

    // File summary
    let _ = writeln!(result, "Changed files:");
    for f in &files {
        if f.line_count > max_lines_per_file {
            let _ = writeln!(
                result,
                "  {} ({} lines, truncated to {})",
                f.path, f.line_count, max_lines_per_file
            );
        } else {
            let _ = writeln!(result, "  {} ({} lines)", f.path, f.line_count);
        }
    }

    // Per-file diffs
    for f in &files {
        result.push('\n');

        if f.line_count <= max_lines_per_file {
            let _ = write!(result, "```diff\n{}\n```", f.text.trim_end());
        } else {
            any_truncated = true;
            let truncated: String = f
                .text
                .lines()
                .take(max_lines_per_file)
                .collect::<Vec<_>>()
                .join("\n");
            let _ = write!(result, "```diff\n{}\n```", truncated.trim_end());
            let _ = write!(
                result,
                "\n[Truncated {}/{} lines for `{}`. Use `paths` to see the full diff.]",
                max_lines_per_file, f.line_count, f.path
            );
        }
    }

    if any_truncated {
        result.push_str(
            "\n\nSome files were truncated. Re-run with `paths` set to the files of interest to \
             see their full diff.",
        );
    }

    result
}

/// Split a unified diff into per-file sections.
fn split_into_files(diff: &str) -> Vec<FileDiff> {
    let mut files = Vec::new();
    let mut current_path = String::new();
    let mut current_lines: Vec<&str> = Vec::new();

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            if !current_path.is_empty() {
                let line_count = current_lines.len();
                files.push(FileDiff {
                    path: std::mem::take(&mut current_path),
                    text: current_lines.join("\n"),
                    line_count,
                });
                current_lines.clear();
            }
            current_path = extract_path(line);
        }
        current_lines.push(line);
    }

    if !current_path.is_empty() {
        let line_count = current_lines.len();
        files.push(FileDiff {
            path: current_path,
            text: current_lines.join("\n"),
            line_count,
        });
    }

    files
}

/// Extract the file path from a `diff --git a/path b/path` line.
fn extract_path(diff_header: &str) -> String {
    // Take the b/ side (the destination path).
    diff_header
        .rsplit_once(" b/")
        .map_or_else(|| diff_header.to_string(), |(_, p)| p.to_string())
}

pub(crate) async fn git_diff(
    root: Utf8PathBuf,
    paths: Option<OneOrMany<String>>,
    status: String,
    options: &Map<String, Value>,
) -> ToolResult {
    let status = DiffStatus::parse(&status)?;
    let paths = paths.unwrap_or_default();
    let env = super::env_from_options(options);
    git_diff_impl(&root, &paths, status, &DuctProcessRunner, &env)
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
