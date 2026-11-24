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
use web_sys::{Element, Range};

use crate::{
    layout::HIGHLIGHTS,
    mapping::{self, Cover, Span},
};

/// The selector of every span of file text inside a file pane.
const SPANS: &str = ".jp-text [data-s]";

/// The class of a span a deletion is marked before.
pub const REMOVED_BEFORE: &str = "jp-removed-before";

/// The class of a span a deletion at the end of the file is marked after.
pub const REMOVED_AFTER: &str = "jp-removed-after";

/// The class a block of the file carries for the frame it is redrawn in.
pub const REPAINT: &str = "jp-repaint";

/// The selector of a block of the file.
const BLOCK: &str = ".jp-text";

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

/// The DOM ranges over the text `spans` show of the bytes `bytes` of `source`,
/// one per span.
#[must_use]
pub fn ranges(spans: &[(Element, Span)], source: &str, bytes: [usize; 2]) -> Vec<Range> {
    let plain: Vec<Span> = spans.iter().map(|(_, span)| *span).collect();
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return vec![];
    };

    mapping::covers(&plain, source, bytes)
        .into_iter()
        .filter_map(|cover| {
            let range = document.create_range().ok()?;
            match cover {
                Cover::Whole { span } => range.select_node_contents(&spans[span].0).ok()?,
                Cover::Part { span, from, to } => {
                    let text = spans[span].0.first_child()?;
                    range.set_start(&text, from).ok()?;
                    range.set_end(&text, to).ok()?;
                }
            }

            Some(range)
        })
        .collect()
}

/// Draw `wanted`, each a highlight name and source byte range of `source`, over
/// the file text in `container`, replacing what was drawn before.
pub fn paint(container: &Element, source: &str, wanted: &[(&'static str, [usize; 2])]) {
    let Some(registry) = registry() else {
        return;
    };
    let spans = spans(container);
    let mut drawn = vec![];

    for name in HIGHLIGHTS {
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
    let node = range.start_container().ok()?;
    let element = match node.dyn_ref::<Element>() {
        Some(element) => element.clone(),
        None => node.parent_element()?,
    };

    element.closest(BLOCK).ok()?
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
