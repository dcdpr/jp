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
    popover::Point,
    selection::Refusal,
};
use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue};
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

/// A file pane holding `html` as a block of the file's text, and `card`, a
/// comment's body, after it.
fn pane(html: &str, card: &str) -> Element {
    let document = web_sys::window().unwrap().document().unwrap();
    let pane = document.create_element("div").unwrap();
    pane.set_inner_html(&format!(
        r#"<div class="jp-text">{html}</div><div class="jp-body">{card}</div>"#
    ));
    document.body().unwrap().append_child(&pane).unwrap();

    pane
}

#[wasm_bindgen_test]
fn a_range_covers_the_text_rendered_from_it() {
    let pane = pane(RENDERED, "");

    // `Some ` and `bold` are set in one paragraph, and drawn as one stretch.
    assert_eq!(covered(&pane, SOURCE, [2, 9]), ["me bo"]);
}

/// The text each range over the bytes `bytes` of `source` covers in `pane`.
fn covered(pane: &Element, source: &str, bytes: [usize; 2]) -> Vec<String> {
    ranges(&spans(pane), source, bytes)
        .iter()
        .map(|range| String::from(range.to_string()))
        .collect()
}

/// Two source lines of a paragraph are a span each, and the line ending between
/// them, which shows as the space between two words, is in neither.
#[wasm_bindgen_test]
fn a_range_covers_the_line_ending_between_two_lines_of_a_paragraph() {
    let html = concat!(
        r#"<p><span data-s="0" data-e="3">one</span>"#,
        "\n",
        r#"<span data-s="4" data-e="7">two</span></p>"#,
    );
    let pane = pane(html, "");

    assert_eq!(covered(&pane, "one\ntwo", [1, 6]), ["ne\ntw"]);
}

/// What lies between two paragraphs is no text of either.
#[wasm_bindgen_test]
fn a_range_across_two_paragraphs_is_one_range_in_each() {
    let html = concat!(
        r#"<p><span data-s="0" data-e="3">one</span></p>"#,
        r#"<div class="jp-plain">between</div>"#,
        r#"<p><span data-s="5" data-e="8">two</span></p>"#,
    );
    let pane = pane(html, "");

    assert_eq!(covered(&pane, "one\n\ntwo", [0, 8]), ["one", "two"]);
}

/// A comment shown under its passage sits in the file's text, in a list item
/// for one, and its own text is rendered by the same renderer.
#[wasm_bindgen_test]
fn the_text_of_what_sits_in_the_file_is_not_the_file() {
    let html = concat!(
        r#"<ul><li><span data-s="2" data-e="5">One</span>"#,
        r#"<div class="jp-plain"><p><span data-s="0" data-e="8">Name it.</span></p></div>"#,
        r#"</li></ul>"#,
    );
    let pane = pane(html, "");

    let found: Vec<[usize; 2]> = spans(&pane)
        .iter()
        .map(|(_, span)| [span.start, span.end])
        .collect();

    assert_eq!(found, [[2, 5]]);
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
    let card = text_in(&pane, ".jp-body [data-s]", 0);

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
    let block = pane.first_element_child().unwrap();
    block
        .insert_adjacent_html(
            "afterend",
            &format!(r#"<div class="jp-text">{second}</div>"#),
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

/// The priority of the highlight the page draws under `name`.
fn priority(name: &str) -> f64 {
    let css = Reflect::get(&js_sys::global(), &JsValue::from_str("CSS")).unwrap();
    let highlights = Reflect::get(&css, &JsValue::from_str("highlights")).unwrap();
    let get: Function = Reflect::get(&highlights, &JsValue::from_str("get"))
        .unwrap()
        .unchecked_into();
    let highlight = get.call1(&highlights, &JsValue::from_str(name)).unwrap();

    Reflect::get(&highlight, &JsValue::from_str("priority"))
        .unwrap()
        .as_f64()
        .unwrap()
}

/// The passage of the comment picked covers the same text as the comment's own
/// highlight, and is the one to show.
#[wasm_bindgen_test]
fn the_passage_picked_is_drawn_over_its_comments_highlight() {
    let pane = pane(RENDERED, "");

    paint(&pane, SOURCE, &[("jp-open", [0, 5]), ("jp-focus", [0, 5])]);

    assert!(priority("jp-focus") > priority("jp-open"));
}

/// The middle of the box the text of `range` is drawn in.
fn middle(range: &Range) -> Point {
    let drawn = range.get_bounding_client_rect();

    Point {
        x: f64::midpoint(drawn.left(), drawn.right()),
        y: f64::midpoint(drawn.top(), drawn.bottom()),
    }
}

#[wasm_bindgen_test]
fn a_point_on_a_passage_hits_the_comments_it_belongs_to() {
    let pane = pane(RENDERED, "");
    let bold = text_in(&pane, ".jp-text [data-s]", 1);
    // `bold` is bytes 7 to 11: comment 1 is on all of the line, comment 2 on
    // `bold`, and comment 3 on ` text`, after it.
    let passages = [(1, [0, 18]), (2, [7, 11]), (3, [13, 18])];

    let hits = capture::hit(&pane, SOURCE, &passages, middle(&range(&bold, 0, &bold, 4)));

    assert_eq!(hits, [(1, [0, 18]), (2, [7, 11])]);
}

#[wasm_bindgen_test]
fn a_point_off_every_passage_hits_nothing() {
    let pane = pane(RENDERED, "");
    let some = text_in(&pane, ".jp-text [data-s]", 0);

    let hits = capture::hit(
        &pane,
        SOURCE,
        &[(2, [7, 11])],
        middle(&range(&some, 0, &some, 4)),
    );

    assert_eq!(hits, Vec::<(u32, [usize; 2])>::new());
}

/// Whether two lengths in CSS pixels are the same, to the pixel.
///
/// A browser places the letters' box on a fraction of a pixel, which the line
/// found from it carries.
fn close(one: f64, other: f64) -> bool {
    (one - other).abs() < 1.0
}

/// The browser draws a selection as tall as its lines, and a range's own boxes
/// are only as tall as its letters: about 12 pixels here, in lines of 30.
#[wasm_bindgen_test]
fn the_lines_of_a_range_are_as_tall_as_the_browser_draws_them_selected() {
    let html = concat!(
        r#"<p style="margin:0;font-size:10px;line-height:30px">"#,
        r#"<span data-s="0" data-e="3">one</span><br>"#,
        r#"<span data-s="4" data-e="7">two</span></p>"#,
    );
    let pane = pane(html, "");
    let one = text_in(&pane, ".jp-text [data-s]", 0);
    let two = text_in(&pane, ".jp-text [data-s]", 1);
    let paragraph = pane.query_selector("p").unwrap().unwrap();
    let drawn = paragraph.get_bounding_client_rect();

    let first = capture::lines(&range(&one, 0, &one, 3)).unwrap();
    let both = capture::lines(&range(&one, 1, &two, 2)).unwrap();

    assert!(close(first.top, drawn.top()), "{first:?}");
    assert!(close(first.bottom, drawn.top() + 30.0), "{first:?}");
    assert!(close(both.top, drawn.top()), "{both:?}");
    assert!(close(both.bottom, drawn.top() + 60.0), "{both:?}");
}

/// A triple click selects up to the start of the next block, where the range
/// has a box with nothing in it.
#[wasm_bindgen_test]
fn a_range_ending_at_the_start_of_a_block_covers_no_line_of_it() {
    let first =
        r#"<p style="margin:0;line-height:30px"><span data-s="0" data-e="3">one</span></p>"#;
    let second =
        r#"<p style="margin:0;line-height:30px"><span data-s="5" data-e="8">two</span></p>"#;
    let pane = pane(&format!("{first}{second}"), "");
    let one = text_in(&pane, ".jp-text [data-s]", 0);
    let two = text_in(&pane, ".jp-text [data-s]", 1);

    let lines = capture::lines(&range(&one, 0, &two, 0)).unwrap();

    assert!(close(lines.bottom - lines.top, 30.0), "{lines:?}");
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
