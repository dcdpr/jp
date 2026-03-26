use std::{borrow::Cow, fmt::Write};

use camino::{Utf8Path, Utf8PathBuf};
use serde_json::{Map, Value};

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
    if let Some(note) = note {
        writeln!(result, "{note}\n")?;
    }
    write!(result, "```diff\n{}\n```", content.trim_end())?;
    Ok(result.into())
}

/// Grep within the diff output, returning matching lines with context.
///
/// Returns `(content, optional_note)` where the note is a summary line like
/// `[Showing X/Y lines...]` meant to be displayed outside the fenced code
/// block.
#[expect(clippy::too_many_lines)]
fn grep_diff<'a>(
    diff: &str,
    pattern: &str,
    context_lines: usize,
) -> Result<(Cow<'a, str>, Option<String>), Box<dyn std::error::Error + Send + Sync>> {
    let regex = fancy_regex::Regex::new(pattern)?;
    let lines: Vec<&str> = diff.lines().collect();
    let line_count = lines.len();

    // Collect indices of matching lines.
    let mut matched = vec![false; line_count];
    for (i, line) in lines.iter().enumerate() {
        if regex.is_match(line)? {
            matched[i] = true;
        }
    }

    let match_count = matched.iter().filter(|&&m| m).count();
    if match_count == 0 {
        return Ok((
            format!("No matches for pattern '{pattern}' in the diff output.").into(),
            None,
        ));
    }

    // Expand context around matches.
    let mut visible = vec![false; line_count];
    for (i, &is_match) in matched.iter().enumerate() {
        if !is_match {
            continue;
        }

        let start = i.saturating_sub(context_lines);
        let end = (i + context_lines + 1).min(line_count);
        for v in &mut visible[start..end] {
            *v = true;
        }
    }

    // Build output, injecting diff structural headers before each region.
    let mut result = String::new();
    let mut last_file_header = None;
    let mut emitted_file_header = None;
    let mut prev_visible = false;

    // Track current file line numbers so we can synthesize accurate
    // @@ headers for each region.
    let mut old_line = 0;
    let mut new_line = 0;

    for (i, line) in lines.iter().enumerate() {
        // Track structural headers.
        if line.starts_with("diff --git ") {
            last_file_header = Some(i);
            old_line = 0;
            new_line = 0;
        } else if line.starts_with("@@ ") {
            let (old_start, new_start) = parse_hunk_start(line);
            old_line = old_start;
            new_line = new_start;
        }

        // Advance line counters for non-visible content lines.
        if !visible[i] {
            match line.as_bytes().first() {
                Some(b'+') => new_line += 1,
                Some(b'-') => old_line += 1,
                Some(b' ') => {
                    old_line += 1;
                    new_line += 1;
                }
                _ => {}
            }
            prev_visible = false;

            continue;
        }

        // At the start of a new region, inject headers.
        if !prev_visible {
            // File header: only when we haven't shown it yet for this file.
            if let Some(fh) = last_file_header
                && emitted_file_header != Some(fh)
                && !visible[fh]
            {
                if !result.is_empty() {
                    result.push('\n');
                }
                for header_line in &lines[fh..i] {
                    if header_line.starts_with("@@ ") {
                        break;
                    }
                    result.push_str(header_line);
                    result.push('\n');
                }
                emitted_file_header = Some(fh);
            }

            // Synthesize a @@ header with current line positions.
            if !line.starts_with("@@ ") && !line.starts_with("diff --git ") {
                let region = &lines[i..];
                let vis = &visible[i..];
                let region_lines: Vec<_> = region
                    .iter()
                    .zip(vis)
                    .take_while(|&(_, &v)| v)
                    .map(|(l, _)| l)
                    .collect();
                let old_count = region_lines
                    .iter()
                    .filter(|l| l.starts_with('-') || l.starts_with(' '))
                    .count();
                let new_count = region_lines
                    .iter()
                    .filter(|l| l.starts_with('+') || l.starts_with(' '))
                    .count();

                writeln!(
                    result,
                    "@@ -{old_line},{old_count} +{new_line},{new_count} @@"
                )?;
            }
        }

        result.push_str(line);
        result.push('\n');
        prev_visible = true;

        // Advance line counters for visible content lines.
        match line.as_bytes().first() {
            Some(b'+') => new_line += 1,
            Some(b'-') => old_line += 1,
            Some(b' ') => {
                old_line += 1;
                new_line += 1;
            }
            _ => {}
        }
    }

    let total_lines = diff.lines().count();
    let visible_lines = visible.iter().filter(|&&v| v).count();
    let note = if visible_lines < total_lines {
        Some(format!(
            "[Showing {visible_lines}/{total_lines} lines matching '{pattern}' ({match_count} \
             matches, {context_lines} lines of context)]"
        ))
    } else {
        None
    };

    Ok((result.into(), note))
}

/// Return the diff, truncating if it exceeds the line limit.
///
/// Returns `(content, optional_note)` — same contract as [`grep_diff`].
fn truncate_diff(diff: &str, max_lines: usize) -> (Cow<'_, str>, Option<String>) {
    let total = diff.lines().count();
    if total <= max_lines {
        return (diff.into(), None);
    }

    let truncated = diff.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    let note = format!(
        "[Showing {max_lines}/{total} lines. Use the `pattern` parameter to search within this \
         diff.]"
    );

    (truncated.into(), Some(note))
}

/// Parse old and new start lines from a `@@` hunk header.
///
/// Format: `@@ -old_start,old_count +new_start,new_count @@`
/// Returns `(old_start, new_start)`, defaulting to 0 on parse failure.
fn parse_hunk_start(hunk_header: &str) -> (usize, usize) {
    let old = parse_hunk_section(hunk_header, '-');
    let new = parse_hunk_section(hunk_header, '+');

    (old, new)
}

fn parse_hunk_section(hunk_header: &str, ch: char) -> usize {
    hunk_header
        .split(ch)
        .nth(1)
        .and_then(|s| s.split([',', ' ']).next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "diff_commit_tests.rs"]
mod tests;
