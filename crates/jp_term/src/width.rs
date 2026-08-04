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

use std::{iter, ops::Range};

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

/// How many clusters past the budget to keep measuring for a ligature that
/// brings the prefix back under it.
///
/// Every width-collapsing rule in `unicode-width` reduces a run of two or three
/// clusters, so a prefix that has overshot recovers within a cluster or two if
/// it recovers at all.
/// Probing a bounded distance keeps a long input from turning the scan
/// quadratic.
const MAX_LIGATURE_PROBES: usize = 8;

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
///
/// Runs in time proportional to the input, with the whole-string measurements
/// needed for ligatures confined to the few clusters around the cut.
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

    let (end, _) = longest_fitting_prefix(s, budget);
    let mut out = s[..end].to_string();
    out.push('…');
    out
}

/// Byte offset just past the longest prefix of `s` that fits `max_width`
/// columns.
///
/// Returns `0` when even the first grapheme cluster is too wide, and `s.len()`
/// when the whole string fits.
/// The offset always falls on a grapheme cluster boundary.
///
/// `s` is expected to carry no ANSI escapes, for the reason given on
/// [`truncate_to_width`].
#[must_use]
pub fn prefix_end_for_width(s: &str, max_width: usize) -> usize {
    longest_fitting_prefix(s, max_width).0
}

/// Byte offset where the longest suffix of `s` that fits `max_width` columns
/// begins.
///
/// Returns `s.len()` when even the last grapheme cluster is too wide, and `0`
/// when the whole string fits.
/// The offset always falls on a grapheme cluster boundary.
///
/// `s` is expected to carry no ANSI escapes, for the reason given on
/// [`truncate_to_width`].
#[must_use]
pub fn suffix_start_for_width(s: &str, max_width: usize) -> usize {
    longest_fitting_suffix(s, max_width).0
}

/// Split `s` into byte ranges, each rendering in at most `max_width` columns.
///
/// Ranges are returned rather than substrings so a caller holding offsets into
/// `s` — match positions, say — can map them onto the row that contains them.
/// They are contiguous except at a break, where the whitespace broken on is
/// left out of both neighbours.
///
/// A row ends at the last whitespace that fits; a word too wide for a row of
/// its own is broken mid-word instead.
/// At least one grapheme cluster is always consumed per row, so a cluster wider
/// than `max_width` overflows its row rather than stalling the split.
///
/// Always returns at least one range: empty input yields one empty range, and a
/// `max_width` of `0` yields the whole string unsplit, since no positive number
/// of columns is available to split it into.
///
/// `s` is expected to carry no ANSI escapes, for the reason given on
/// [`truncate_to_width`].
#[must_use]
pub fn wrap_ranges(s: &str, max_width: usize) -> Vec<Range<usize>> {
    if s.is_empty() || max_width == 0 {
        return iter::once(0..s.len()).collect();
    }

    let mut rows = Vec::new();
    let mut start = 0;

    while start < s.len() {
        let rest = &s[start..];
        if display_width(rest) <= max_width {
            rows.push(start..s.len());
            return rows;
        }

        let mut end = prefix_end_for_width(rest, max_width);
        if end == 0 {
            // A single cluster wider than the whole row. It has to go somewhere,
            // and leaving it for the next row would never terminate.
            end = first_cluster_end(rest);
        }

        // The widest word break the budget allows. The budget boundary itself
        // counts when the character there is whitespace, which is the case of a
        // word ending exactly at the row edge.
        let word_break = if rest[end..].starts_with(char::is_whitespace) {
            Some(end)
        } else {
            rest[..end].rfind(char::is_whitespace)
        };

        // Whitespace before the break belongs to the break, not to the row.
        // A break with nothing but whitespace ahead of it — an indent wider than
        // the row — would make no progress, so the row is cut mid-word instead.
        if let Some(at) = word_break
            .map(|at| rest[..at].trim_end().len())
            .filter(|at| *at > 0)
        {
            end = at;
        }

        rows.push(start..start + end);

        // The whitespace broken on is consumed by the break itself. When the
        // break was mid-word there is none, and nothing is skipped.
        let tail = &s[start + end..];
        start += end + (tail.len() - tail.trim_start().len());
    }

    rows
}

/// Byte offset just past the first grapheme cluster of `s`.
///
/// `s.len()` when `s` is empty.
fn first_cluster_end(s: &str) -> usize {
    s.grapheme_indices(true)
        .next()
        .map_or(s.len(), |(at, cluster)| at + cluster.len())
}

/// Byte offset just past the longest prefix of `s` that fits `budget` columns,
/// and the number of whole-string measurements taken to find it.
///
/// The count is returned so tests can pin it: it has to depend on `budget`, not
/// on the size of `s`, and measuring every candidate prefix instead turns a
/// long input quadratic.
fn longest_fitting_prefix(s: &str, budget: usize) -> (usize, usize) {
    // The running sum of cluster widths is an upper bound on the true width of
    // the prefix: the interactions that make a string narrower than its parts
    // (ZWJ emoji sequences, Arabic Lam-Alef, Tifinagh joiners) span cluster
    // boundaries, while the ones that make it wider (a quotation mark plus
    // U+FE01) stay inside a single cluster and are caught by measuring the
    // cluster whole. So a prefix that fits under the sum certainly fits, and
    // that costs O(1) per cluster.
    //
    // Only once the sum passes the budget does the exact width matter, and then
    // it takes a whole-string measurement, because a longer prefix can render
    // narrower than a shorter one: a Tifinagh consonant joiner costs a column on
    // its own and none once the consonant after it completes the ligature.
    let mut end = 0;
    let mut sum = 0;
    let mut probes = 0;
    let mut measurements = 0;

    for (offset, cluster) in s.grapheme_indices(true) {
        let candidate = offset + cluster.len();
        sum += UnicodeWidthStr::width(cluster);

        if sum <= budget {
            end = candidate;
            continue;
        }

        if probes == MAX_LIGATURE_PROBES {
            break;
        }
        probes += 1;
        measurements += 1;

        if UnicodeWidthStr::width(&s[..candidate]) <= budget {
            end = candidate;
            // A ligature closed and brought the prefix back under budget; allow
            // a fresh run of probes for the next one.
            probes = 0;
        }
    }

    (end, measurements)
}

/// Byte offset where the longest suffix of `s` that fits `budget` columns
/// begins, and the number of whole-string measurements taken to find it.
///
/// The mirror of [`longest_fitting_prefix`], scanning clusters from the end,
/// and resting on the same bound: a running sum of cluster widths over-counts a
/// ligature but never under-counts, so a suffix that fits under the sum
/// certainly fits.
fn longest_fitting_suffix(s: &str, budget: usize) -> (usize, usize) {
    let mut start = s.len();
    let mut sum = 0;
    let mut probes = 0;
    let mut measurements = 0;

    for (offset, cluster) in s.grapheme_indices(true).rev() {
        sum += UnicodeWidthStr::width(cluster);

        if sum <= budget {
            start = offset;
            continue;
        }

        if probes == MAX_LIGATURE_PROBES {
            break;
        }
        probes += 1;
        measurements += 1;

        if UnicodeWidthStr::width(&s[offset..]) <= budget {
            start = offset;
            probes = 0;
        }
    }

    (start, measurements)
}

#[cfg(test)]
#[path = "width_tests.rs"]
mod tests;
