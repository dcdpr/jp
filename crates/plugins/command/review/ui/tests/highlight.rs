//! Source ranges mapped onto a rendered file in a real DOM, and DOM selections
//! mapped back to source ranges, in a headless browser.
//!
//! Run with `cargo test --target wasm32-unknown-unknown --test highlight`, and
//! `wasm-bindgen-test-runner` as the target's runner; the host has no DOM, and
//! builds this file empty.

#![cfg(target_arch = "wasm32")]

use jp_review_ui::{
    capture,
    highlight::{REMOVED_AFTER, REMOVED_BEFORE, REPAINT, mark_removed, paint, ranges, spans},
    selection::Refusal,
};
use js_sys::Promise;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::{Element, Node, Range};

wasm_bindgen_test_configure!(run_in_browser);

/// `Some **bold** text`, as `md-html` renders it.
const SOURCE: &str = "Some **bold** text";

const RENDERED: &str = concat!(
    r#"<p><span data-s="0" data-e="5">Some </span>"#,
    r#"<strong><span data-s="7" data-e="11">bold</span></strong>"#,
    r#"<span data-s="13" data-e="18"> text</span></p>"#,
);

/// A file pane holding `html` as the file's text, and `card` beside it.
fn pane(html: &str, card: &str) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    let pane = document.create_element("div").unwrap();
    pane.set_inner_html(&format!(
        r#"<div class="jp-row"><div class="jp-text">{html}</div><div class="jp-cards">{card}</div></div>"#
    ));
    document.body().unwrap().append_child(&pane).unwrap();

    pane
}

#[wasm_bindgen_test]
fn a_range_covers_the_text_rendered_from_it() {
    let pane = pane(RENDERED, "");

    let covered: Vec<String> = ranges(&spans(&pane), SOURCE, [2, 9])
        .iter()
        .map(|range| String::from(range.to_string()))
        .collect();

    assert_eq!(covered, ["me ", "bo"]);
}

/// A comment body is rendered by the same renderer, and its offsets are into
/// the comment.
#[wasm_bindgen_test]
fn the_text_of_a_card_is_not_the_file() {
    let pane = pane(
        RENDERED,
        r#"<p><span data-s="0" data-e="8">Name it.</span></p>"#,
    );

    let found: Vec<[usize; 2]> = spans(&pane)
        .iter()
        .map(|(_, span)| [span.start, span.end])
        .collect();

    assert_eq!(found, [[0, 5], [7, 11], [13, 18]]);
}

/// The text node inside the `index`th element `selector` matches in `pane`.
fn text_in(pane: &Element, selector: &str, index: u32) -> Node {
    pane.query_selector_all(selector)
        .unwrap()
        .item(index)
        .unwrap()
        .first_child()
        .unwrap()
}

/// A range from `offset` into `start` to `end_offset` into `end`.
fn range(start: &Node, offset: u32, end: &Node, end_offset: u32) -> Range {
    let range = web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .create_range()
        .unwrap();
    range.set_start(start, offset).unwrap();
    range.set_end(end, end_offset).unwrap();

    range
}

#[wasm_bindgen_test]
fn a_selection_maps_back_to_the_source_bytes_it_covers() {
    let pane = pane(RENDERED, "");
    let some = text_in(&pane, ".jp-text [data-s]", 0);
    let bold = text_in(&pane, ".jp-text [data-s]", 1);

    // `me ` of `Some ` through `bo` of `bold`.
    let bytes = capture::anchor(&pane, SOURCE, &range(&some, 2, &bold, 2));

    assert_eq!(bytes, Some(Ok([2, 9])));
}

/// A selection ending in text the renderer made up, such as a list marker
/// written without a source range, can't be anchored.
#[wasm_bindgen_test]
fn a_selection_ending_in_unmapped_text_is_refused() {
    let html = concat!(
        r#"<p><span data-s="0" data-e="5">Some </span>"#,
        r#"<span class="unmapped">made up</span></p>"#,
    );
    let pane = pane(html, "");
    let some = text_in(&pane, ".jp-text [data-s]", 0);
    let made_up = text_in(&pane, ".unmapped", 0);

    let refused = capture::anchor(&pane, "Some ", &range(&some, 1, &made_up, 3));

    assert_eq!(refused, Some(Err(Refusal::Unmapped)));
}

/// Text selected in a comment card is the card's, not the file's.
#[wasm_bindgen_test]
fn a_selection_in_a_card_is_not_in_the_file() {
    let pane = pane(
        RENDERED,
        r#"<p><span data-s="0" data-e="8">Name it.</span></p>"#,
    );
    let card = text_in(&pane, ".jp-cards [data-s]", 0);

    assert_eq!(
        capture::anchor(&pane, SOURCE, &range(&card, 0, &card, 4)),
        None
    );
}

/// Safari leaves a highlight that moved painted where it was, so every block of
/// the file a highlight left or reached is drawn again.
#[wasm_bindgen_test]
async fn moving_a_highlight_redraws_the_blocks_it_left_and_reached() {
    let source = "First line.\n\nSecond line.";
    let first = r#"<p><span data-s="0" data-e="11">First line.</span></p>"#;
    let second = r#"<p><span data-s="13" data-e="25">Second line.</span></p>"#;
    let pane = pane(first, "");
    let row = pane.first_element_child().unwrap();
    row.insert_adjacent_html(
        "afterend",
        &format!(r#"<div class="jp-row"><div class="jp-text">{second}</div></div>"#),
    )
    .unwrap();
    let blocks = pane.query_selector_all(".jp-text").unwrap();
    let redrawn = || -> Vec<bool> {
        (0..blocks.length())
            .map(|index| {
                let block: Element = blocks.item(index).unwrap().unchecked_into();
                block.class_list().contains(REPAINT)
            })
            .collect()
    };

    paint(&pane, source, &[("jp-pending", [0, 5])]);
    assert_eq!(redrawn(), [true, false]);
    frames(3).await;
    assert_eq!(redrawn(), [false, false], "the redraw lasts a frame");

    paint(&pane, source, &[("jp-pending", [13, 19])]);
    assert_eq!(redrawn(), [true, true]);
    frames(3).await;

    paint(&pane, source, &[]);
    assert_eq!(redrawn(), [false, true]);
}

/// Wait `count` animation frames.
async fn frames(count: u32) {
    for _ in 0..count {
        let frame = Promise::new(&mut |resolve, _| {
            web_sys::window()
                .unwrap()
                .request_animation_frame(&resolve)
                .unwrap();
        });
        JsFuture::from(frame).await.unwrap();
    }
}

#[wasm_bindgen_test]
fn a_deletion_is_marked_on_the_text_after_it_or_the_last() {
    let pane = pane(RENDERED, "");
    let found = spans(&pane);

    mark_removed(&found, &[6, 40]);

    let classes: Vec<String> = found
        .iter()
        .map(|(element, _)| element.class_name())
        .collect();
    assert_eq!(classes, [
        String::new(),
        REMOVED_BEFORE.to_owned(),
        REMOVED_AFTER.to_owned()
    ]);
}
