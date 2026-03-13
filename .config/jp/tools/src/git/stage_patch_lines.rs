use camino::Utf8Path;
use serde_json::{Map, Value};

use super::apply::{apply_patch_to_index, build_patch};
use crate::util::{
    ToolResult,
    runner::{DuctProcessRunner, ProcessOutput, ProcessRunner},
};

pub(crate) fn git_stage_patch_lines(
    root: &Utf8Path,
    path: &str,
    patch_id: usize,
    lines: Vec<Value>,
    options: &Map<String, Value>,
) -> ToolResult {
    let lines = parse_line_selectors(lines)?;
    let env = super::env_from_options(options);
    git_stage_patch_lines_impl(root, path, patch_id, &lines, &DuctProcessRunner, &env)
}

fn git_stage_patch_lines_impl<R: ProcessRunner>(
    root: &Utf8Path,
    path: &str,
    patch_id: usize,
    lines: &[usize],
    runner: &R,
    env: &[(&str, &str)],
) -> ToolResult {
    if lines.is_empty() {
        return Err("No lines selected for staging.".into());
    }

    let hunk = fetch_hunk(root, path, patch_id, runner, env)?;
    let sub_hunk = build_sub_hunk(&hunk, lines)?;
    let patch = build_patch(path, &sub_hunk);

    apply_patch_to_index(&patch, root, runner, env)?;
    Ok("Patch applied.".into())
}

/// A parsed diff line from a `--unified=0` hunk.
#[derive(Debug, Clone)]
struct DiffLine {
    kind: DiffLineKind,
    content: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DiffLineKind {
    Removal,
    Addition,
}

/// Parsed hunk header coordinates.
#[derive(Debug)]
struct HunkHeader {
    old_start: usize,
}

fn parse_hunk_header(header: &str) -> Result<HunkHeader, String> {
    // Format: "@@ -OLD[,COUNT] +NEW[,COUNT] @@..."
    let parts: Vec<_> = header.split_whitespace().collect();
    let old_part = parts
        .get(1)
        .ok_or("Invalid hunk header: missing old range")?;

    let old_range = old_part.trim_start_matches('-');
    let (old_start, _old_count) = parse_range(old_range)?;

    Ok(HunkHeader { old_start })
}

fn parse_range(range: &str) -> Result<(usize, usize), String> {
    let parts: Vec<_> = range.split(',').collect();
    let start: usize = parts[0]
        .parse()
        .map_err(|_| format!("Invalid line number: {}", parts[0]))?;
    let count: usize = if parts.len() > 1 {
        parts[1]
            .parse()
            .map_err(|_| format!("Invalid line count: {}", parts[1]))?
    } else {
        1
    };

    Ok((start, count))
}

/// Fetch a specific hunk from the working tree diff.
fn fetch_hunk<R: ProcessRunner>(
    root: &Utf8Path,
    path: &str,
    patch_id: usize,
    runner: &R,
    env: &[(&str, &str)],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
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
        return Err(format!("Failed to get diff for '{path}': {stderr}").into());
    }

    // Split on `\n@@ ` and skip the diff header (everything before the first
    // hunk). Each segment after skip(1) lacks the `@@ ` prefix, so we re-add
    // it.
    stdout
        .split("\n@@ ")
        .skip(1)
        .nth(patch_id)
        .map(|h| format!("@@ {h}"))
        .ok_or_else(|| {
            format!(
                "Patch {patch_id} not found for '{path}'. Run git_list_patches to see available \
                 patches."
            )
            .into()
        })
}

/// Parse a hunk into its header and individual diff lines.
fn parse_hunk(hunk: &str) -> Result<(HunkHeader, Vec<DiffLine>), String> {
    let mut lines_iter = hunk.lines();
    let header_line = lines_iter.next().ok_or("Empty hunk")?;
    let header = parse_hunk_header(header_line)?;

    let diff_lines: Vec<_> = lines_iter
        .filter_map(|line| {
            if let Some(content) = line.strip_prefix('-') {
                Some(DiffLine {
                    kind: DiffLineKind::Removal,
                    content: content.to_string(),
                })
            } else {
                line.strip_prefix('+').map(|content| DiffLine {
                    kind: DiffLineKind::Addition,
                    content: content.to_string(),
                })
            }
        })
        .collect();

    Ok((header, diff_lines))
}

/// Build a sub-hunk from a parsed hunk, keeping only the selected line indices.
///
/// Selected `-` lines become removals. Selected `+` lines become additions.
/// Unselected lines are dropped — those changes remain in the working tree.
///
/// All selected lines go into a single sub-hunk because a `--unified=0` hunk
/// covers one contiguous region of the old file. Removals are emitted first
/// (preserving order), then additions — matching git's unified diff format.
fn build_sub_hunk(hunk: &str, selected: &[usize]) -> Result<String, String> {
    let (header, diff_lines) = parse_hunk(hunk)?;

    if diff_lines.is_empty() {
        return Err("Hunk contains no diff lines.".into());
    }

    let max_index = diff_lines.len() - 1;
    for &idx in selected {
        if idx > max_index {
            return Err(format!(
                "Line index {idx} is out of range (hunk has lines 0..={max_index})."
            ));
        }
    }

    let mut sorted: Vec<usize> = selected.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    // Find the old-file start position. This is the position of the first
    // selected removal, or the hunk's original start if only additions are
    // selected.
    let mut old_start = header.old_start;
    let mut old_pos = header.old_start;
    let mut found_removal = false;

    for (i, line) in diff_lines.iter().enumerate() {
        if line.kind == DiffLineKind::Removal {
            if !found_removal && sorted.contains(&i) {
                old_start = old_pos;
                found_removal = true;
            }

            old_pos += 1;
        }
    }

    // Build the body: removals first, then additions (git's format).
    let mut removals = 0;
    let mut additions = 0;
    let mut body = String::new();

    for &idx in &sorted {
        let line = &diff_lines[idx];
        if !matches!(line.kind, DiffLineKind::Removal) {
            continue;
        }

        removals += 1;
        body.push('-');
        body.push_str(&line.content);
        body.push('\n');
    }

    for &idx in &sorted {
        let line = &diff_lines[idx];
        if matches!(line.kind, DiffLineKind::Removal) {
            continue;
        }

        additions += 1;
        body.push('+');
        body.push_str(&line.content);
        body.push('\n');
    }

    let hunk_header = format!("@@ -{old_start},{removals} +{old_start},{additions} @@");
    Ok(format!("{hunk_header}\n{body}"))
}

/// Expands a mixed array of line selectors into a flat list of indices.
///
/// Each element is either:
/// - An integer (single line index, e.g. `42`)
/// - A string range with inclusive bounds (e.g. `"1:50"`)
fn parse_line_selectors(values: Vec<Value>) -> Result<Vec<usize>, String> {
    let mut indices = vec![];
    for value in values {
        match value {
            Value::Number(n) => {
                let idx = n
                    .as_u64()
                    .ok_or_else(|| format!("Invalid line index: {n}"))?;
                let idx =
                    usize::try_from(idx).map_err(|_| format!("Line index too large: {idx}"))?;

                indices.push(idx);
            }
            Value::String(s) => {
                let (start, end) = parse_range_selector(&s)?;

                indices.extend(start..=end);
            }
            other => {
                return Err(format!(
                    "Invalid line selector: {other}. Expected an integer or a range string like \
                     \"1:50\"."
                ));
            }
        }
    }

    Ok(indices)
}

fn parse_range_selector(s: &str) -> Result<(usize, usize), String> {
    let Some((left, right)) = s.split_once(':') else {
        return Err(format!(
            "Invalid range format '{s}'. Expected 'start:end' (e.g. '1:50')."
        ));
    };

    let start: usize = left
        .parse()
        .map_err(|_| format!("Invalid range start: '{left}'"))?;

    let end: usize = right
        .parse()
        .map_err(|_| format!("Invalid range end: '{right}'"))?;

    if start > end {
        return Err(format!(
            "Invalid range '{s}': start ({start}) must be <= end ({end})."
        ));
    }

    Ok((start, end))
}

#[cfg(test)]
#[path = "stage_patch_lines_tests.rs"]
mod tests;
