//! Where in a rendered file an element sits that is about a range of the file,
//! as the composer is while it writes about one: in the flow of the text, right
//! under the text the range ends in.
//!
//! Under a paragraph, a heading, a table, or a code block, that is after the
//! block.
//! In a list it is inside the list item the range ends in, under the item's own
//! text and over the lists nested in it: a list is one block however many items
//! it holds, and the end of a long one is far from most of its items.
//!
//! The element is moved to its seat rather than drawn there, since the file's
//! text is HTML the server rendered, which each block holds as it came.
//! Elements can share a seat, each saying where among the others it sits
//! ([`Order`]).

use wasm_bindgen::JsCast;
use web_sys::{Element, Node};

use crate::highlight::{self, BLOCK, PLAIN};

/// The selector of a list item.
const ITEM: &str = "li";

/// The selector of a list nested right in the element it is asked of.
const NESTED: &str = ":scope > ul, :scope > ol";

/// A place in the text of a rendered file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seat {
    /// Inside the list item `item`, before the list `nested` in it, or at its
    /// end when it holds none.
    Item {
        item: Element,
        nested: Option<Element>,
    },

    /// Right after a block of the file.
    Block(Element),
}

/// Where an element sits among those that share its seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    /// Right under the text, before the others.
    First,

    /// After the others.
    Last,
}

/// The seat under the text the bytes `bytes` of `source` end in, in the file
/// drawn in `pane`; `None` when none of their text is drawn.
#[must_use]
pub fn under(pane: &Element, source: &str, bytes: [usize; 2]) -> Option<Seat> {
    let spans = highlight::spans(pane);
    let last = highlight::ranges(&spans, source, bytes).pop()?;
    let element = highlight::element_of(&last.end_container().ok()?)?;

    let item = element
        .closest(ITEM)
        .ok()
        .flatten()
        .filter(|item| pane.contains(Some(item)));
    if let Some(item) = item {
        let nested = item.query_selector(NESTED).ok().flatten();
        return Some(Seat::Item { item, nested });
    }

    element.closest(BLOCK).ok().flatten().map(Seat::Block)
}

/// The byte of the source at which the text `seat` is under ends: that of the
/// block, or of the list item's own text, without the lists nested in it.
///
/// `None` when the seat is under no text of the file.
#[must_use]
pub fn end(seat: &Seat) -> Option<usize> {
    let (within, nested) = match seat {
        Seat::Item { item, nested } => (item, nested.as_ref()),
        Seat::Block(block) => (block, None),
    };

    highlight::spans(within)
        .into_iter()
        .filter(|(element, _)| !nested.is_some_and(|nested| nested.contains(Some(element))))
        .map(|(_, span)| span.end)
        .max()
}

/// Move `host` to `seat`, `order` among the elements seated there, unless it
/// sits there already.
///
/// Moving an element takes the keyboard from a field in it, so one that sits
/// where it should is left alone.
pub fn take(host: &Element, seat: &Seat, order: Order) {
    let host: &Node = host;
    let is_seated = |node: &Node| node == host || plain(node);
    let (parent, end): (Option<Node>, Option<Node>) = match seat {
        Seat::Item { item, nested } => (Some(item.clone().into()), nested.clone().map(Into::into)),
        Seat::Block(block) => {
            let mut next = block.next_sibling();
            while let Some(node) = next.as_ref().filter(|node| is_seated(node)) {
                next = node.next_sibling();
            }
            (block.parent_node(), next)
        }
    };
    let Some(parent) = parent else {
        return;
    };

    // The elements seated before `end`, last to first.
    let mut seated = vec![];
    let mut previous = match &end {
        Some(end) => end.previous_sibling(),
        None => parent.last_child(),
    };
    while let Some(node) = previous.filter(|node| is_seated(node)) {
        previous = node.previous_sibling();
        seated.push(node);
    }
    let (there, before) = match order {
        Order::First => (seated.last(), seated.last().or(end.as_ref())),
        Order::Last => (seated.first(), end.as_ref()),
    };
    if there == Some(host) {
        return;
    }

    // A seat that can't take the host leaves it where it was, still usable.
    drop(parent.insert_before(host, before));
}

/// Whether `node` is an element seated in the file's text, and no part of it.
fn plain(node: &Node) -> bool {
    node.dyn_ref::<Element>()
        .is_some_and(|element| element.class_list().contains(PLAIN))
}

/// Move `host` out of the file's text, to the end of `home`, unless it is
/// there.
pub fn park(host: &Element, home: &Element) {
    let home: &Node = home;
    if host.parent_node().as_ref() == Some(home) {
        return;
    }

    drop(home.append_child(host));
}
