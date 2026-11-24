//! The browser's text selection, read as the source bytes of the file it
//! covers.
//!
//! [`read`] says where each end of the selection falls among the file's
//! rendered spans, and `crate::selection` turns that into bytes.
//! Text in comment cards is rendered by the same renderer, but only the spans
//! inside `.jp-text` are the file's.

use leptos_dom::helpers::window;
use web_sys::{Element, Node, Range};

use crate::{
    highlight,
    mapping::Span,
    selection::{self, End, Refusal},
};

/// The class `md-html` gives text it wrote without a source range.
const UNMAPPED: &str = "unmapped";

/// The source bytes of `source` that the selection covers in `pane`, the file
/// pane; `None` while nothing is selected, or the selection holds none of the
/// file's text.
///
/// # Errors
///
/// The selection is refused when an end is in unmapped text, or it covers no
/// rendered text.
#[must_use]
pub fn read(pane: &Element, source: &str) -> Option<Result<[usize; 2], Refusal>> {
    let selection = window().get_selection().ok()??;
    if selection.is_collapsed() || selection.range_count() == 0 {
        return None;
    }
    let range = selection.get_range_at(0).ok()?;

    anchor(pane, source, &range)
}

/// Remove the browser's text selection.
pub fn clear() {
    if let Ok(Some(selection)) = window().get_selection() {
        drop(selection.remove_all_ranges());
    }
}

/// The source bytes of `source` that `range` covers in `pane`; `None` when it
/// holds none of the file's text.
#[must_use]
pub fn anchor(pane: &Element, source: &str, range: &Range) -> Option<Result<[usize; 2], Refusal>> {
    let spans = highlight::spans(pane);
    let covered: Vec<usize> = spans
        .iter()
        .enumerate()
        .filter(|(_, (element, _))| range.intersects_node(element).unwrap_or(false))
        .map(|(index, _)| index)
        .collect();
    let (first, last) = (*covered.first()?, *covered.last()?);

    let start = end(
        pane,
        &range.start_container().ok()?,
        range.start_offset().ok()?,
        &spans,
        first,
    );
    let end = end(
        pane,
        &range.end_container().ok()?,
        range.end_offset().ok()?,
        &spans,
        last,
    );
    let plain: Vec<Span> = spans.iter().map(|(_, span)| *span).collect();

    Some(selection::anchor(&plain, source, start, end))
}

/// Where the end of a selection at `offset` into `container` falls among
/// `spans`; `beyond` is the span it falls past when not in one's text.
fn end(
    pane: &Element,
    container: &Node,
    offset: u32,
    spans: &[(Element, Span)],
    beyond: usize,
) -> End {
    let parent = match container.parent_element() {
        Some(parent) if container.node_type() == Node::TEXT_NODE => parent,
        _ => return End::Beyond { span: beyond },
    };
    if parent.class_list().contains(UNMAPPED) && pane.contains(Some(&parent)) {
        return End::Unmapped;
    }

    match spans.iter().position(|(element, _)| *element == parent) {
        Some(span) => End::In { span, offset },
        None => End::Beyond { span: beyond },
    }
}
