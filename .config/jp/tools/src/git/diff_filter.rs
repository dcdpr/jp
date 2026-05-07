//! Shared diff-rendering helpers used by `git_diff_commit` and `git_diff_file`.
//!
//! - [`truncate_diff`] caps a diff's total line count.
//! - [`grep_diff`] greps within a diff and returns matching lines with
//!   surrounding context, synthesizing per-region hunk headers so the output
//!   is still a valid-looking unified diff.
//!
//! Both functions return `(content, optional_note)` where the note is meant
//! to be displayed outside the fenced code block.

use std::{borrow::Cow, fmt::Write};

/// Grep within the diff output, returning matching lines with context.
///
/// Returns `(content, optional_note)` where the note is a summary line like
/// `[Showing X/Y lines...]` meant to be displayed outside the fenced code
/// block.
#[expect(clippy::too_many_lines)]
pub(super) fn grep_diff<'a>(
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
pub(super) fn truncate_diff(diff: &str, max_lines: usize) -> (Cow<'_, str>, Option<String>) {
    let total = diff.lines().count();
    if total <= max_lines {
        return (diff.into(), None);
    }

    let truncated = diff.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    let note = format!(
        "[Showing {max_lines}/{total} lines. Use `pattern` to search, or `start_line` / \
         `end_line` to page through this diff.]"
    );

    (truncated.into(), Some(note))
}

/// Validate user-provided line range arguments.
///
/// Checks the static cross-cuts: both bounds must be positive, and `start`
/// must not exceed `end`. The bound-vs-content check (`start > total_lines`)
/// happens in the caller, since it depends on the diff's actual size and the
/// error message wants to include that count.
pub(super) fn validate_line_range(
    start: Option<usize>,
    end: Option<usize>,
) -> Result<(), &'static str> {
    if start.is_some_and(|v| v == 0) {
        return Err("`start_line` must be greater than 0.");
    }
    if end.is_some_and(|v| v == 0) {
        return Err("`end_line` must be greater than 0.");
    }
    if let (Some(s), Some(e)) = (start, end)
        && s > e
    {
        return Err("`start_line` must be less than or equal to `end_line`.");
    }
    Ok(())
}

/// Slice the diff to a 1-based output-line range, returning just the
/// extracted body without markers.
///
/// `start_line` and `end_line` are inclusive 1-based offsets into the diff's
/// rendered output. Markers are added separately by [`add_slice_markers`]
/// after any other processing (grep, truncate) so the slice context is still
/// visible even when the body has been further filtered.
///
/// Validation (positive bounds, ordering, `start <= total`) must happen in
/// the caller before this is invoked.
pub(super) fn slice_diff(diff: &str, start_line: Option<usize>, end_line: Option<usize>) -> String {
    let lines: Vec<&str> = diff.lines().collect();
    let total = lines.len();

    let start_idx = start_line.map_or(0, |v| v.saturating_sub(1));
    let end_idx = end_line.map_or(total, |v| v.min(total));

    let slice: &[&str] = if start_idx < end_idx {
        &lines[start_idx..end_idx]
    } else {
        &[]
    };
    slice.join("\n")
}

/// Wrap content with `fs_read_file`-style range markers.
///
/// Mirrors `fs_read_file`'s output shape:
/// ```text
/// ... (starting from line #N) ...
/// <content>
/// ... (truncated after line #M) ...
/// ```
///
/// Applied as the last step so that markers survive intermediate processing
/// (e.g. when [`grep_diff`] is run inside the slice and would otherwise
/// strip them as non-matching lines).
pub(super) fn add_slice_markers(
    content: &mut String,
    start_line: Option<usize>,
    end_line: Option<usize>,
) {
    if let Some(s) = start_line {
        content.insert_str(0, &format!("... (starting from line #{s}) ...\n"));
    }
    if let Some(e) = end_line {
        content.push_str(&format!("\n... (truncated after line #{e}) ..."));
    }
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
#[path = "diff_filter_tests.rs"]
mod tests;
