//! Source byte ranges, mapped onto the text `md-html` rendered from them.
//!
//! Every run of rendered text sits in a span carrying the byte range of the
//! source it came from, `data-s` to `data-e`.
//! A plain span's text matches its source character for character, so a byte
//! offset inside it maps to an offset into its text node.
//! An atom span (`data-atom`) stands for its source as a whole, such as `|`
//! rendered from `\|`, so a range touching it covers it whole.
//! Offsets into a text node count UTF-16 code units, as a DOM `Range` does.

/// What one rendered span says of the source its text came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,

    /// Whether the span's text stands for its source as a whole.
    pub atom: bool,
}

/// The part of one span's text a source range covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    /// The text from one UTF-16 offset up to another.
    Part { span: usize, from: u32, to: u32 },

    /// All of the span's text.
    Whole { span: usize },
}

impl Cover {
    /// The index of the span covered.
    #[must_use]
    pub fn span(self) -> usize {
        match self {
            Self::Part { span, .. } | Self::Whole { span } => span,
        }
    }
}

/// Where a deletion is marked: before the span's text, or after it when nothing
/// follows the deleted lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub span: usize,
    pub after: bool,
}

/// The text each of `spans` shows of the half-open byte range `bytes` of
/// `source`.
///
/// A span whose range doesn't lie on character boundaries of `source` is left
/// out, as one rendered from another source would be.
#[must_use]
pub fn covers(spans: &[Span], source: &str, [start, end]: [usize; 2]) -> Vec<Cover> {
    spans
        .iter()
        .enumerate()
        .filter_map(|(index, span)| {
            let (from, to) = (start.max(span.start), end.min(span.end));
            if from >= to {
                return None;
            }
            if span.atom {
                return Some(Cover::Whole { span: index });
            }

            Some(Cover::Part {
                span: index,
                from: utf16(source.get(span.start..from)?),
                to: utf16(source.get(span.start..to)?),
            })
        })
        .collect()
}

/// The runs of `covers` drawn as one stretch of text each, as half-open ranges
/// of indexes into `covers`: neighbours that cover consecutive spans, which
/// `joined` says are set in the same lines of text.
///
/// What lies between two spans of a run is drawn with them: the line ending of
/// the source, which shows as the space between two words, and text the
/// renderer wrote without a source range.
#[must_use]
pub fn runs(covers: &[Cover], joined: impl Fn(usize, usize) -> bool) -> Vec<[usize; 2]> {
    let mut runs: Vec<[usize; 2]> = vec![];
    for (index, cover) in covers.iter().enumerate() {
        let before = index.checked_sub(1).map(|before| covers[before].span());
        let continues =
            before.is_some_and(|before| before + 1 == cover.span() && joined(before, cover.span()));
        match runs.last_mut() {
            Some(run) if continues => run[1] = index + 1,
            _ => runs.push([index, index + 1]),
        }
    }

    runs
}

/// How many of the lines `added` are not shown by any of `spans`: lines with
/// text that renders to nothing, such as a link reference definition.
///
/// A blank line shows nothing either, and isn't counted.
#[must_use]
pub fn hidden(spans: &[Span], source: &str, added: &[[usize; 2]]) -> usize {
    added
        .iter()
        .filter(|[start, end]| {
            let text = source.get(*start..*end).unwrap_or_default();
            !text.trim().is_empty() && covers(spans, source, [*start, *end]).is_empty()
        })
        .count()
}

/// Where to mark each deletion in `removed`, byte offsets where removed lines
/// were: before the first span at or after the offset, or after the last span
/// when none is.
#[must_use]
pub fn marks(spans: &[Span], removed: &[usize]) -> Vec<Mark> {
    let mut marks: Vec<Mark> = vec![];
    for &offset in removed {
        let mark = match spans.iter().position(|span| span.start >= offset) {
            Some(span) => Mark { span, after: false },
            None if spans.is_empty() => continue,
            None => Mark {
                span: spans.len() - 1,
                after: true,
            },
        };
        if !marks.contains(&mark) {
            marks.push(mark);
        }
    }

    marks
}

/// The length of `text` in UTF-16 code units, saturating at `u32::MAX`.
fn utf16(text: &str) -> u32 {
    u32::try_from(text.encode_utf16().count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
