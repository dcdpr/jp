//! Display-width measurement and truncation for terminal output.
//!
//! Widths are counted in terminal columns, not bytes or `char`s: ANSI styling
//! and OSC 8 hyperlinks are stripped before measuring, and East Asian wide
//! characters count as two columns.
//!
//! Use these whenever output has to fit a known terminal width.
//! Measuring with `str::len` counts bytes and overstates any non-ASCII string;
//! counting `char`s understates wide characters and ignores escape sequences
//! entirely.

use strip_ansi_escapes::strip_str;
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr;

/// Display width of `s` in terminal columns.
///
/// ANSI styling and OSC 8 hyperlinks are removed first, so a styled string
/// measures as its visible text only.
#[must_use]
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(strip_str(s).as_str())
}

/// Display width of the widest line in `rendered`.
///
/// Returns `0` for empty input.
#[must_use]
pub fn max_line_width(rendered: &str) -> usize {
    rendered.lines().map(display_width).max().unwrap_or(0)
}

/// Truncate `s` to at most `max_width` display columns, appending `…` when
/// cut.
///
/// The ellipsis is counted against the budget, so the result never exceeds
/// `max_width` columns.
/// A `max_width` of `0` yields an empty string.
///
/// The cut falls on a grapheme cluster boundary, so an emoji ZWJ or modifier
/// sequence is either kept whole or dropped whole rather than left with a
/// trailing joiner.
///
/// `s` is expected to carry no ANSI escapes: the budget is spent per cluster,
/// so escape bytes would consume it and could be split mid-sequence.
/// Truncate before styling, not after.
#[must_use]
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if display_width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }

    // Reserve one column for the ellipsis.
    let budget = max_width - 1;
    let mut width = 0;
    let mut out = String::new();

    // Measured per cluster rather than per scalar: the scalars of a ZWJ emoji
    // sequence sum to twice the width the sequence renders in, which would
    // spend the budget at twice the true rate and could cut between a base
    // character and its joiner.
    for cluster in s.graphemes(true) {
        let w = UnicodeWidthStr::width(cluster);
        if width + w > budget {
            break;
        }
        width += w;
        out.push_str(cluster);
    }
    out.push('…');
    out
}

#[cfg(test)]
#[path = "width_tests.rs"]
mod tests;
