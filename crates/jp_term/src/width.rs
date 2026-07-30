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
/// Input that already fits is returned unchanged, which includes zero-column
/// input under a `max_width` of `0`; anything wider than a `max_width` of `0`
/// yields an empty string.
///
/// The cut falls on a grapheme cluster boundary, so an emoji ZWJ or modifier
/// sequence is either kept whole or dropped whole rather than left with a
/// trailing joiner.
///
/// `s` is expected to carry no ANSI escapes: the budget is spent on the text,
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

    // Each candidate prefix is measured whole rather than by summing the widths
    // of its clusters: a ZWJ emoji sequence's clusters sum to twice the width
    // the sequence renders in, and Arabic Lam-Alef spans two clusters that sum
    // to two columns but ligate into one.
    //
    // Every boundary is measured, without stopping at the first prefix that
    // overshoots, because a longer prefix can render narrower than a shorter
    // one: a Tifinagh consonant joiner costs a column on its own and none once
    // the consonant after it completes the ligature.
    let mut end = 0;
    for (offset, cluster) in s.grapheme_indices(true) {
        let candidate = offset + cluster.len();
        if UnicodeWidthStr::width(&s[..candidate]) <= budget {
            end = candidate;
        }
    }

    let mut out = s[..end].to_string();
    out.push('…');
    out
}

#[cfg(test)]
#[path = "width_tests.rs"]
mod tests;
