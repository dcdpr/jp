//! The browser's text selection, read as the source bytes of the file it
//! covers, and a point of the window, read as the comments' passages under it.
//!
//! [`read`] says where each end of the selection falls among the file's
//! rendered spans, and `crate::selection` turns that into bytes.
//! Text in comment cards is rendered by the same renderer, but only the spans
//! inside `.jp-text` are the file's.
//!
//! [`hit`] goes the other way, from source bytes to where their text is drawn:
//! a highlight is no element of the page, so nothing but its place says a click
//! fell on one.

use leptos_dom::helpers::window;
use web_sys::{DomRect, Element, Node, Range};

use crate::{
    highlight,
    mapping::Span,
    popover::{Point, Rect},
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

/// Whether the browser holds a selection of text.
#[must_use]
pub fn selecting() -> bool {
    window()
        .get_selection()
        .ok()
        .flatten()
        .is_some_and(|selection| !selection.is_collapsed())
}

/// The box around the lines the browser's selection covers, in the window, and
/// whether the selection was made backward, from its end to its start; `None`
/// while nothing is selected.
///
/// The box is as tall as the browser draws the selection: the whole of each
/// line, which is taller than the letters on it.
#[must_use]
pub fn shape() -> Option<(Rect, bool)> {
    let selection = window().get_selection().ok()??;
    if selection.is_collapsed() || selection.range_count() == 0 {
        return None;
    }
    let range = selection.get_range_at(0).ok()?;
    // The focus is the end of the selection that moves; a range always runs
    // forward.
    let backward = selection.focus_node() == range.start_container().ok()
        && Some(selection.focus_offset()) == range.start_offset().ok();

    Some((lines(&range)?, backward))
}

/// The box around the lines `range` covers text on; `None` when it covers none.
///
/// A range's own boxes are as tall as its letters.
/// Each is grown to the height of its line, by the line height of the text at
/// that end of the range.
#[must_use]
pub fn lines(range: &Range) -> Option<Rect> {
    let boxes = range.get_client_rects()?;
    // A range ending at the start of a block, as a triple click makes one,
    // has an empty box there, on a line it selects nothing of.
    let drawn: Vec<DomRect> = (0..boxes.length())
        .filter_map(|index| boxes.item(index))
        .filter(|drawn| drawn.width() > 0.0 && drawn.height() > 0.0)
        .collect();
    let (first, last) = (drawn.first()?, drawn.last()?);
    let above = leading(&range.start_container().ok()?, first.height());
    let below = leading(&range.end_container().ok()?, last.height());

    let mut bounds = Rect {
        left: first.left(),
        top: first.top(),
        right: first.right(),
        bottom: first.bottom(),
    };
    for drawn in &drawn {
        bounds.left = bounds.left.min(drawn.left());
        bounds.top = bounds.top.min(drawn.top());
        bounds.right = bounds.right.max(drawn.right());
        bounds.bottom = bounds.bottom.max(drawn.bottom());
    }
    bounds.top -= above;
    bounds.bottom += below;

    Some(bounds)
}

/// How far a line of the text in `node` reaches past letters `height` tall, on
/// either side of them: half of what the line height adds.
fn leading(node: &Node, height: f64) -> f64 {
    let line_height = highlight::element_of(node)
        .and_then(|element| window().get_computed_style(&element).ok().flatten())
        .and_then(|style| style.get_property_value("line-height").ok())
        // `normal`, which is no number, is about as tall as the letters.
        .and_then(|value| value.trim_end_matches("px").parse::<f64>().ok())
        .unwrap_or(height);

    f64::midpoint(0.0, line_height - height).max(0.0)
}

/// Those of `passages`, each a comment's number and its source bytes of
/// `source`, whose text in `pane` is drawn under the point `at` of the window.
#[must_use]
pub fn hit(
    pane: &Element,
    source: &str,
    passages: &[(u32, [usize; 2])],
    at: Point,
) -> Vec<(u32, [usize; 2])> {
    let spans = highlight::spans(pane);

    passages
        .iter()
        .copied()
        .filter(|(_, bytes)| {
            highlight::ranges(&spans, source, *bytes)
                .iter()
                .any(|range| drawn_under(range, at))
        })
        .collect()
}

/// Whether one of the boxes the text of `range` is drawn in holds `at`; text
/// that wraps is drawn in a box per line.
fn drawn_under(range: &Range, at: Point) -> bool {
    let Some(boxes) = range.get_client_rects() else {
        return false;
    };

    (0..boxes.length())
        .filter_map(|index| boxes.item(index))
        .any(|drawn| {
            (drawn.left()..=drawn.right()).contains(&at.x)
                && (drawn.top()..=drawn.bottom()).contains(&at.y)
        })
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
