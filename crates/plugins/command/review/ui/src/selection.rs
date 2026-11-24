//! A selection in a rendered file, as the half-open range of source bytes it
//! covers.
//!
//! The browser side (`crate::capture`) says where each end of the selection
//! falls relative to the spans `md-html` wrapped the file's text in; this
//! module turns that into bytes.
//! An end inside a plain span maps character for character; an end inside an
//! atom span (`data-atom`) snaps to the atom's boundary, the start of the
//! selection to the atom's start and its end to the atom's end.

use crate::mapping::{self, Span};

/// Where one end of a selection falls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// In the text of span `span`, `offset` UTF-16 code units in.
    In { span: usize, offset: u32 },

    /// Outside the text of every span; `span` is the first span the selection
    /// covers, for its start, or the last, for its end.
    Beyond { span: usize },

    /// In text the renderer wrote without a source range.
    Unmapped,
}

/// Why a selection can't be anchored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// It covers none of the file's rendered text.
    Empty,

    /// One of its ends is in text without a source range.
    Unmapped,
}

impl Refusal {
    /// Why the selection can't take a comment, as the comment bar says it.
    #[must_use]
    pub fn reason(self) -> &'static str {
        match self {
            Self::Empty => "The selection covers none of the file's text.",
            Self::Unmapped => {
                "The selection starts or ends in text the renderer made up, which has no place in \
                 the file to anchor a comment to. Select a little more or less."
            }
        }
    }
}

/// How long a press on the comment bar keeps a selection that ends, in
/// milliseconds from when the press began.
pub const PRESS_WINDOW: f64 = 1000.0;

/// Whether a selection that ended at `now` keeps the range it captured, where
/// `pressed` is when the last press on the comment bar began.
///
/// Only a press on the bar keeps it, since a tap on a touch screen can end the
/// selection before the button sees the tap.
/// Any other way the selection ends drops the range and its highlight.
#[must_use]
pub fn kept(pressed: Option<f64>, now: f64) -> bool {
    pressed.is_some_and(|pressed| (0.0..PRESS_WINDOW).contains(&(now - pressed)))
}

/// Which end of a selection a position is.
#[derive(Clone, Copy)]
enum Side {
    Start,
    End,
}

/// The bytes of `source` a selection from `start` to `end` covers, where
/// `spans` are the file's rendered spans in document order.
///
/// # Errors
///
/// Refused when either end is in unmapped text, and when the range holds no
/// rendered text: a selection across Markdown syntax alone is empty.
pub fn anchor(spans: &[Span], source: &str, start: End, end: End) -> Result<[usize; 2], Refusal> {
    let from = position(spans, source, start, Side::Start)?;
    let to = position(spans, source, end, Side::End)?;
    if from >= to || mapping::covers(spans, source, [from, to]).is_empty() {
        return Err(Refusal::Empty);
    }

    Ok([from, to])
}

/// The byte of `source` that `end` falls at, as the `side` of a selection.
fn position(spans: &[Span], source: &str, end: End, side: Side) -> Result<usize, Refusal> {
    let (span, offset) = match end {
        End::Unmapped => return Err(Refusal::Unmapped),
        End::Beyond { span } => (span, None),
        End::In { span, offset } => (span, Some(offset)),
    };
    let span = spans.get(span).ok_or(Refusal::Empty)?;
    let edge = match side {
        Side::Start => span.start,
        Side::End => span.end,
    };
    let Some(offset) = offset.filter(|_| !span.atom) else {
        return Ok(edge);
    };
    let text = source.get(span.start..span.end).ok_or(Refusal::Empty)?;

    Ok(span.start + utf8_offset(text, offset))
}

/// The byte offset into `text` of its `utf16`th UTF-16 code unit; an offset
/// between the two halves of a surrogate pair rounds down to the character's
/// start.
fn utf8_offset(text: &str, utf16: u32) -> usize {
    let mut units = 0_u32;
    for (index, character) in text.char_indices() {
        let width = if character.len_utf16() == 1 { 1 } else { 2 };
        if units + width > utf16 {
            return index;
        }
        units += width;
    }

    text.len()
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
