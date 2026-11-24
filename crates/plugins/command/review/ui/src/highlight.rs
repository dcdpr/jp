//! Highlighting source ranges in a rendered file without changing its DOM,
//! through the CSS Custom Highlight API.
//!
//! `web-sys` gates the API behind `web_sys_unstable_apis`, so the two types it
//! needs are bound here directly.
//! A browser without the API (before Safari 17.2, Chrome 105, Firefox 140) is
//! left without highlights, and the comment cards still show.
//!
//! The file's text is read from the `[data-s]` spans inside `.jp-text`
//! elements, which hold the file; comment bodies rendered by the same renderer
//! sit outside them and are left alone.
//!
//! Safari doesn't redraw the text a highlight stops covering: the old color
//! stays on screen, whole or as a sliver above the letters, until something
//! else redraws it.
//! So [`paint`] and [`clear`] give every `.jp-text` block a highlight left or
//! reached the class [`REPAINT`] for one frame, which the stylesheet turns into
//! a change Safari redraws the block for.

use std::cell::RefCell;

use js_sys::Reflect;
use leptos_dom::helpers::request_animation_frame;
use wasm_bindgen::{JsCast, JsValue, prelude::wasm_bindgen};
use web_sys::{Element, Node, Range};

use crate::{
    layout::HIGHLIGHTS,
    mapping::{self, Cover, Span},
};

/// The class of what sits in a file's text without being part of it, such as a
/// comment shown under its passage, which the stylesheet keeps the text's
/// styles off.
pub const PLAIN: &str = "jp-plain";

/// The selector of every span of file text inside a file pane.
///
/// What sits in the file's text without being part of it ([`PLAIN`]) holds
/// spans of its own text.
const SPANS: &str = ".jp-text [data-s]:not(.jp-plain [data-s])";

/// The selector of an element whose text is set in lines together: a paragraph,
/// a heading, a list item's own text, a table cell, a code block, or a line of
/// a plain-text file.
const FLOW: &str = "p, li, h1, h2, h3, h4, h5, h6, td, th, dt, dd, pre, .jp-text";

/// The class of a span a deletion is marked before.
pub const REMOVED_BEFORE: &str = "jp-removed-before";

/// The class of a span a deletion at the end of the file is marked after.
pub const REMOVED_AFTER: &str = "jp-removed-after";

/// The class a block of the file carries for the frame it is redrawn in.
pub const REPAINT: &str = "jp-repaint";

/// The selector of a block of the file.
pub(crate) const BLOCK: &str = ".jp-text";

thread_local! {
    /// The blocks of the file the highlights last drawn cover.
    static PAINTED: RefCell<Vec<Element>> = const { RefCell::new(Vec::new()) };
}

#[wasm_bindgen]
extern "C" {
    /// A set of ranges, styled by the `::highlight()` rule of the name it is
    /// registered under.
    type Highlight;

    #[wasm_bindgen(constructor, catch)]
    fn new() -> Result<Highlight, JsValue>;

    #[wasm_bindgen(method)]
    fn add(this: &Highlight, range: &Range);

    /// Where two highlights cover the same text, the one with the higher
    /// priority is drawn over the other.
    #[wasm_bindgen(method, setter)]
    fn set_priority(this: &Highlight, priority: u32);

    /// `CSS.highlights`, the highlights the page draws by name.
    type HighlightRegistry;

    #[wasm_bindgen(method)]
    fn set(this: &HighlightRegistry, name: &str, highlight: &Highlight);

    #[wasm_bindgen(method)]
    fn delete(this: &HighlightRegistry, name: &str) -> bool;
}

/// `CSS.highlights`; `None` in a browser without the API.
fn registry() -> Option<HighlightRegistry> {
    let css = Reflect::get(&js_sys::global(), &JsValue::from_str("CSS")).ok()?;
    let highlights = Reflect::get(&css, &JsValue::from_str("highlights")).ok()?;
    if highlights.is_undefined() || highlights.is_null() {
        return None;
    }

    Some(highlights.unchecked_into())
}

/// Every span of file text inside `container`, in document order, with the
/// source range it says it came from.
#[must_use]
pub fn spans(container: &Element) -> Vec<(Element, Span)> {
    let Ok(found) = container.query_selector_all(SPANS) else {
        return vec![];
    };

    (0..found.length())
        .filter_map(|index| found.item(index)?.dyn_into::<Element>().ok())
        .filter_map(|element| {
            let number = |name: &str| element.get_attribute(name)?.parse().ok();
            let span = Span {
                start: number("data-s")?,
                end: number("data-e")?,
                atom: element.has_attribute("data-atom"),
            };

            Some((element, span))
        })
        .collect()
}

/// The DOM ranges over the text `spans` show of the bytes `bytes` of `source`.
///
/// Spans set in the same lines of text ([`FLOW`]) share one range, which also
/// covers what lies between them: a line ending of the source, shown as the
/// space between two words, belongs to no span.
#[must_use]
pub fn ranges(spans: &[(Element, Span)], source: &str, bytes: [usize; 2]) -> Vec<Range> {
    let plain: Vec<Span> = spans.iter().map(|(_, span)| *span).collect();
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return vec![];
    };
    let covers = mapping::covers(&plain, source, bytes);
    let flow = |span: usize| spans[span].0.closest(FLOW).ok().flatten();

    mapping::runs(&covers, |one, other| flow(one) == flow(other))
        .into_iter()
        .filter_map(|[first, end]| {
            let range = document.create_range().ok()?;
            let (node, offset) = edge(spans, covers[first], false)?;
            range.set_start(&node, offset).ok()?;
            let (node, offset) = edge(spans, covers[end - 1], true)?;
            range.set_end(&node, offset).ok()?;

            Some(range)
        })
        .collect()
}

/// The DOM position `cover` starts at in its span of `spans`, or the one it
/// ends at for `end`.
fn edge(spans: &[(Element, Span)], cover: Cover, end: bool) -> Option<(Node, u32)> {
    match cover {
        Cover::Part { span, from, to } => {
            let text = spans[span].0.first_child()?;
            Some((text, if end { to } else { from }))
        }
        Cover::Whole { span } => {
            let element: &Node = &spans[span].0;
            let offset = if end {
                element.child_nodes().length()
            } else {
                0
            };
            Some((element.clone(), offset))
        }
    }
}

/// Draw `wanted`, each a highlight name and source byte range of `source`, over
/// the file text in `container`, replacing what was drawn before.
///
/// Where two highlights cover the same text, the one later in [`HIGHLIGHTS`] is
/// drawn over the other.
pub fn paint(container: &Element, source: &str, wanted: &[(&'static str, [usize; 2])]) {
    let Some(registry) = registry() else {
        return;
    };
    let spans = spans(container);
    let mut drawn = vec![];

    for (priority, name) in (0_u32..).zip(HIGHLIGHTS) {
        let ranges: Vec<Range> = wanted
            .iter()
            .filter(|(wanted, _)| *wanted == name)
            .flat_map(|(_, bytes)| ranges(&spans, source, *bytes))
            .collect();
        for block in ranges.iter().filter_map(block_of) {
            if !drawn.contains(&block) {
                drawn.push(block);
            }
        }
        let highlight = match Highlight::new() {
            Ok(highlight) if !ranges.is_empty() => highlight,
            _ => {
                registry.delete(name);
                continue;
            }
        };
        for range in &ranges {
            highlight.add(range);
        }
        highlight.set_priority(priority);
        registry.set(name, &highlight);
    }

    let mut changed = PAINTED.with(|painted| painted.replace(drawn.clone()));
    for block in drawn {
        if !changed.contains(&block) {
            changed.push(block);
        }
    }
    redraw(changed);
}

/// The block of the file `range` is in.
fn block_of(range: &Range) -> Option<Element> {
    element_of(&range.start_container().ok()?)?
        .closest(BLOCK)
        .ok()?
}

/// `node` when it is an element, and otherwise the element holding it, as the
/// one a text node is in.
#[must_use]
pub fn element_of(node: &Node) -> Option<Element> {
    match node.dyn_ref::<Element>() {
        Some(element) => Some(element.clone()),
        None => node.parent_element(),
    }
}

/// Make Safari draw `blocks` again: each carries [`REPAINT`] for a frame.
fn redraw(blocks: Vec<Element>) {
    if blocks.is_empty() {
        return;
    }
    for block in &blocks {
        // A block that can't take the class keeps whatever Safari last drew.
        drop(block.class_list().add_1(REPAINT));
    }

    drop(request_animation_frame(move || {
        for block in &blocks {
            drop(block.class_list().remove_1(REPAINT));
        }
    }));
}

/// Mark where lines were removed, before the text that follows them, or after
/// the file's last text when nothing does.
pub fn mark_removed(spans: &[(Element, Span)], removed: &[usize]) {
    let plain: Vec<Span> = spans.iter().map(|(_, span)| *span).collect();

    for mark in mapping::marks(&plain, removed) {
        let class = if mark.after {
            REMOVED_AFTER
        } else {
            REMOVED_BEFORE
        };
        // A class that can't be added leaves the deletion unmarked; the
        // unified diff still shows it.
        drop(spans[mark.span].0.class_list().add_1(class));
    }
}

/// Stop drawing every highlight the UI draws.
pub fn clear() {
    let Some(registry) = registry() else {
        return;
    };

    for name in HIGHLIGHTS {
        registry.delete(name);
    }
    redraw(PAINTED.with(RefCell::take));
}
