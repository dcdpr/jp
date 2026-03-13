use std::{cmp::min, fs};

use camino::Utf8Path;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    to_simple_xml_with_root,
    util::{
        OneOrMany, ToolResult,
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
    },
};

#[derive(Debug, Serialize)]
struct Patch {
    path: String,
    id: String,
    diff: String,
}

#[derive(Debug, Serialize)]
struct Warning {
    message: String,
}

#[derive(Serialize)]
struct Output {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<Warning>,
    patches: Vec<Patch>,
}

pub(crate) fn git_list_patches(
    root: &Utf8Path,
    files: OneOrMany<String>,
    options: &Map<String, Value>,
) -> ToolResult {
    let env = super::env_from_options(options);
    git_list_patches_impl(root, files, &DuctProcessRunner, &env)
}

fn git_list_patches_impl<R: ProcessRunner>(
    root: &Utf8Path,
    files: OneOrMany<String>,
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    let mut patches = vec![];
    let mut warnings = vec![];

    for path in files {
        let path = path.trim();

        let ProcessOutput {
            stdout,
            stderr,
            status,
        } = runner.run_with_env(
            "git",
            &["diff-files", "-p", "--minimal", "--unified=0", "--", path],
            root,
            env,
        )?;

        if !status.is_success() {
            warnings.push(Warning {
                message: format!("Failed to list patches for '{path}': {stderr}"),
            });

            continue;
        }

        // See: <https://www.gnu.org/software/diffutils/manual/diffutils.html#Detailed-Unified>
        let Some((_, tail)) = stdout.split_once("\n@@ ") else {
            if stdout.is_empty() && !root.join(path).exists() {
                warnings.push(Warning {
                    message: format!("File not found: {path}"),
                });
            }

            // No changes for this file.
            continue;
        };

        // For deleted files the working tree copy is gone, so context lines
        // will be empty. For existing files we read them for pretty-printing
        // context around each hunk.
        let file_content = fs::read_to_string(root.join(path)).unwrap_or_default();
        let source_lines: Vec<_> = file_content.lines().collect();

        let mut tail = tail.to_string();
        tail.insert_str(0, "@@ ");

        for (id, hunk) in tail.split("\n@@ ").enumerate() {
            let hunk_with_header = format!("@@ {hunk}");

            patches.push(Patch {
                path: path.to_string(),
                id: id.to_string(),
                diff: pretty_print_diff(&hunk_with_header, hunk, &source_lines),
            });
        }
    }

    if warnings.is_empty() {
        to_simple_xml_with_root(&patches, "patches").map(Into::into)
    } else {
        let output = Output { warnings, patches };
        to_simple_xml_with_root(&output, "patches").map(Into::into)
    }
}

/// Pretty print a git diff hunk with numbered change lines.
///
/// Context lines (from the source file) get padding to align with the `[N] `
/// prefix on diff lines. Actual diff lines (`-`/`+`) are prefixed with `[N]`
/// where N is a sequential index used by `git_stage_patch_lines` to select
/// individual lines for staging.
fn pretty_print_diff(hunk_with_header: &str, hunk: &str, source_lines: &[&str]) -> String {
    // Parse the header to find coordinates.
    let parts: Vec<_> = hunk_with_header.split_whitespace().collect();

    // Find part starting with '+' (target file coordinates).
    let new_file_part = parts.iter().find(|p| p.starts_with('+')).unwrap_or(&"+0,0");
    let coords: Vec<_> = new_file_part.trim_start_matches('+').split(',').collect();

    let start_line: usize = coords[0].parse().unwrap_or(0);
    let count: usize = if coords.len() > 1 {
        coords[1].parse().unwrap_or(0)
    } else {
        1
    };

    // Count diff lines to determine the padding width for alignment.
    let diff_line_count = hunk.lines().skip(1).count();
    let max_index = diff_line_count.saturating_sub(1);
    let index_prefix_width = format!("[{max_index}] ").len();

    // Calculate context indices (0-indexed).
    let line_idx = if start_line > 0 { start_line - 1 } else { 0 };

    // 3 lines before.
    let ctx_before_start = line_idx.saturating_sub(3);
    let ctx_before_end = line_idx;

    // 3 lines after.
    let hunk_end_idx = line_idx + count;
    let ctx_after_start = hunk_end_idx;
    let ctx_after_end = min(source_lines.len(), hunk_end_idx + 3);

    let mut result = String::new();
    let mut line_index = 0;

    // Pre-context.
    for i in ctx_before_start..ctx_before_end {
        if let Some(line) = source_lines.get(i) {
            push_context_line(&mut result, line, index_prefix_width);
        }
    }

    // Actual changes — number each `-`/`+` line.
    //
    // Skip the first line of raw_body, which contains the header info (e.g.,
    // "-1,1 +1,1 @@").
    for line in hunk.lines().skip(1) {
        let prefix = format!("[{line_index}] ");
        let padding = index_prefix_width - prefix.len();
        result.push_str(&" ".repeat(padding));
        result.push_str(&prefix);
        result.push_str(line);
        result.push('\n');
        line_index += 1;
    }

    // Post-context.
    for i in ctx_after_start..ctx_after_end {
        if let Some(line) = source_lines.get(i) {
            push_context_line(&mut result, line, index_prefix_width);
        }
    }

    result.trim_end().to_string()
}

fn push_context_line(result: &mut String, line: &str, index_prefix_width: usize) {
    // Pad to align with `[N] -` / `[N] +` prefixed diff lines.
    result.push_str(&" ".repeat(index_prefix_width));
    result.push(' ');
    result.push_str(line);
    result.push('\n');
}

#[cfg(test)]
#[path = "list_patches_tests.rs"]
mod tests;
