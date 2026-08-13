//! Line-based text diffing shared by tools that report file changes.
//!
//! Two output shapes, both built from [`text_diff`]:
//!
//! - [`unified_diff`] produces a standard unified diff, meant to be wrapped in
//!   a ` ```diff ` fence so the terminal renderer highlights it and the
//!   assistant reads it as plain text.
//! - [`colored_diff`] produces an ANSI-styled, line-numbered rendering for
//!   direct terminal output.

use std::{fmt::Write as _, time::Duration};

use crossterm::style::{Color, ContentStyle, Stylize as _};
use similar::{ChangeTag, TextDiff, udiff::UnifiedDiff};

/// Diff two texts by line, using the Patience algorithm.
///
/// Diffing gives up after two seconds and falls back to a coarser result, so
/// pathological inputs cannot stall a tool call.
pub(crate) fn text_diff<'old, 'new, 'bufs>(
    old: &'old str,
    new: &'new str,
) -> TextDiff<'old, 'new, 'bufs, str> {
    similar::TextDiff::configure()
        .algorithm(similar::Algorithm::Patience)
        .timeout(Duration::from_secs(2))
        .diff_lines(old, new)
}

/// Build a unified diff with three lines of context, headed by `file`.
pub(crate) fn unified_diff<'diff, 'old, 'new, 'bufs>(
    diff: &'diff TextDiff<'old, 'new, 'bufs, str>,
    file: &str,
) -> UnifiedDiff<'diff, 'old, 'new, 'bufs, str> {
    let mut unified = diff.unified_diff();
    unified.context_radius(3).header(file, file);
    unified
}

/// Formats a line number as a right-aligned string of the given width, or blank
/// spaces if the index is `None`.
fn fmt_line_num(index: Option<usize>, width: usize) -> String {
    match index {
        Some(idx) => format!("{:>width$}", idx + 1),
        None => " ".repeat(width),
    }
}

/// Render a diff as ANSI-styled terminal output.
///
/// Each line carries its old and new line numbers, changed words within a line
/// are highlighted, and the insertion/deletion counts are repeated above and
/// below the body so they stay visible on long diffs.
pub(crate) fn colored_diff<'old, 'new, 'diff: 'old + 'new, 'bufs>(
    diff: &'diff TextDiff<'old, 'new, 'bufs, str>,
    unified: &UnifiedDiff<'diff, 'old, 'new, 'bufs, str>,
    path: &str,
) -> String {
    let mut buf = String::new();

    let (additions, deletions) =
        diff.iter_all_changes()
            .fold((0, 0), |(mut add, mut del), change| {
                if matches!(change.tag(), ChangeTag::Delete) {
                    del += 1;
                } else if matches!(change.tag(), ChangeTag::Insert) {
                    add += 1;
                }
                (add, del)
            });

    // Dynamic number column width based on the largest line number.
    let max_line = diff.old_slices().len().max(diff.new_slices().len()).max(1);
    let nw = max_line.to_string().len();

    // Build stats: deletions first (left column = red), additions second (right = green).
    let mut stats_plain = String::new();
    let mut stats_colored = String::new();
    if deletions > 0 {
        stats_plain.push_str(&format!("-{deletions}"));
        stats_colored.push_str(format!("-{deletions}").red().to_string().as_str());
    }
    if additions > 0 {
        if !stats_plain.is_empty() {
            stats_plain.push(',');
            stats_colored.push(',');
        }
        stats_plain.push_str(&format!("+{additions}"));
        stats_colored.push_str(format!("+{additions}").green().to_string().as_str());
    }
    let stats_width = stats_plain.len();

    // Unified column where │ sits. Enough room for two right-aligned number
    // columns plus a separator space, or the stats text plus a leading space.
    let line_nums_width = 2 * nw + 1;
    let pipe_col = (line_nums_width + 1).max(stats_width + 1);

    // Header: stats line + separator.
    let stats_pad = " ".repeat(pipe_col - stats_width - 1);
    let header_line = format!("{stats_pad}{stats_colored} │ {}\n", path.bold());
    let separator = format!("{}┼{}\n", "─".repeat(pipe_col), "─".repeat(path.len() + 2));
    buf.push_str(&header_line);
    buf.push_str(&separator);

    // Hunks, with an ellipsis separator between non-contiguous regions.
    let num_pad = " ".repeat(pipe_col - line_nums_width);
    let mut first_hunk = true;
    for hunk in unified.iter_hunks() {
        if !first_hunk {
            let _ = writeln!(&mut buf, "{}│ …", " ".repeat(pipe_col));
        }
        first_hunk = false;

        for op in hunk.ops() {
            for change in diff.iter_inline_changes(op) {
                let (sign, s) = match change.tag() {
                    ChangeTag::Delete => ("-", ContentStyle::new().red()),
                    ChangeTag::Insert => ("+", ContentStyle::new().green()),
                    ChangeTag::Equal => (" ", ContentStyle::new().dim()),
                };

                // Emphasized (word-level) spans keep the line's foreground and
                // add a dark background in the same hue (256-color cube: 52 =
                // dark red, 22 = dark green), so changed words read as a
                // highlight of the line's own color on any terminal theme.
                let em = match change.tag() {
                    ChangeTag::Delete => s.on(Color::AnsiValue(52)),
                    ChangeTag::Insert => s.on(Color::AnsiValue(22)),
                    ChangeTag::Equal => s,
                };

                let old = fmt_line_num(change.old_index(), nw);
                let new = fmt_line_num(change.new_index(), nw);

                let _ = write!(
                    &mut buf,
                    "{} {}{}│{}",
                    s.apply(old),
                    s.apply(new),
                    num_pad,
                    s.apply(sign).bold(),
                );
                for (emphasized, value) in change.iter_strings_lossy() {
                    if emphasized {
                        let _ = write!(&mut buf, "{}", em.apply(value));
                    } else {
                        let _ = write!(&mut buf, "{}", s.apply(value));
                    }
                }
                if change.missing_newline() {
                    buf.push('\n');
                }
            }
        }
    }

    // Footer: separator + stats (mirrored header for long diffs).
    buf.push_str(&separator);
    buf.push_str(&header_line);

    buf
}
