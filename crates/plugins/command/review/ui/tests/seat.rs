//! The composer's element seated in a rendered file, in a real DOM, in a
//! headless browser.
//!
//! Run with `cargo test --target wasm32-unknown-unknown --test seat`, and
//! `wasm-bindgen-test-runner` as the target's runner; the host has no DOM, and
//! builds this file empty.

#![cfg(target_arch = "wasm32")]

use jp_review_ui::seat::{Order, end, park, take, under};
use wasm_bindgen::JsCast;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
use web_sys::{Document, Element, HtmlElement};

wasm_bindgen_test_configure!(run_in_browser);

/// `First.` and `Second.`, two paragraphs, as `md-html` renders each.
const PARAGRAPHS: &str = "First.\n\nSecond.\n";

const FIRST: &str = r#"<p><span data-s="0" data-e="6">First.</span></p>"#;

const SECOND: &str = r#"<p><span data-s="8" data-e="15">Second.</span></p>"#;

/// A list of `One`, holding a list of `Inner`, and `Two`.
const LIST: &str = "- One\n  - Inner\n- Two\n";

/// [`LIST`] rendered, each item named by a class.
///
/// The tests share one page, so an item is looked up in its own pane.
const LIST_HTML: &str = concat!(
    r#"<ul><li class="one"><span data-s="2" data-e="5">One</span>"#,
    r#"<ul><li class="inner"><span data-s="10" data-e="15">Inner</span></li></ul></li>"#,
    r#"<li class="two"><span data-s="18" data-e="21">Two</span></li></ul>"#,
);

fn document() -> Document {
    web_sys::window().unwrap().document().unwrap()
}

/// A file pane holding `blocks`, each the HTML of one block of the file.
fn pane(blocks: &[&str]) -> Element {
    let pane = document().create_element("div").unwrap();
    for block in blocks {
        pane.insert_adjacent_html(
            "beforeend",
            &format!(r#"<div class="jp-text">{block}</div>"#),
        )
        .unwrap();
    }
    document().body().unwrap().append_child(&pane).unwrap();

    pane
}

/// An element to seat, holding a field, in a home of its own outside any file.
fn host() -> (Element, Element) {
    let home = document().create_element("div").unwrap();
    home.set_inner_html(r#"<div class="host"><textarea></textarea></div>"#);
    document().body().unwrap().append_child(&home).unwrap();
    let host = home.first_element_child().unwrap();

    (home, host)
}

/// The children of `parent`, each as its tag and class.
fn children(parent: &Element) -> Vec<String> {
    let mut found = vec![];
    let mut next = parent.first_element_child();
    while let Some(child) = next {
        found.push(format!("{}.{}", child.tag_name(), child.class_name()));
        next = child.next_element_sibling();
    }

    found
}

#[wasm_bindgen_test]
fn under_a_paragraph_is_after_its_block() {
    let pane = pane(&[FIRST, SECOND]);
    let (_home, host) = host();

    // `First`, without its full stop.
    let seat = under(&pane, PARAGRAPHS, [0, 5]).unwrap();
    take(&host, &seat, Order::First);

    assert_eq!(children(&pane), ["DIV.jp-text", "DIV.host", "DIV.jp-text"]);
}

/// The end of a range decides: one from the first paragraph into the second
/// sits under the second.
#[wasm_bindgen_test]
fn under_a_range_across_blocks_is_after_the_last() {
    let pane = pane(&[FIRST, SECOND]);
    let (_home, host) = host();

    let seat = under(&pane, PARAGRAPHS, [2, 12]).unwrap();
    take(&host, &seat, Order::First);

    assert_eq!(children(&pane), ["DIV.jp-text", "DIV.jp-text", "DIV.host"]);
}

/// A list is one block, however long: the seat is in the item, under its own
/// text and over the list nested in it.
#[wasm_bindgen_test]
fn under_a_list_items_text_is_inside_the_item_before_its_nested_list() {
    let pane = pane(&[LIST_HTML]);
    let (_home, host) = host();

    let seat = under(&pane, LIST, [2, 5]).unwrap();
    take(&host, &seat, Order::First);

    let one = pane.query_selector(".one").unwrap().unwrap();
    assert_eq!(children(&one), ["SPAN.", "DIV.host", "UL."]);
}

#[wasm_bindgen_test]
fn under_a_nested_items_text_is_at_the_end_of_that_item() {
    let pane = pane(&[LIST_HTML]);
    let (_home, host) = host();

    let seat = under(&pane, LIST, [10, 15]).unwrap();
    take(&host, &seat, Order::First);

    let inner = pane.query_selector(".inner").unwrap().unwrap();
    assert_eq!(children(&inner), ["SPAN.", "DIV.host"]);
}

/// The read marker of a blank line goes under the text before it, which is what
/// was read: the blank line between the paragraphs is line 2, bytes 7 to 7.
#[wasm_bindgen_test]
fn the_text_up_to_a_blank_line_ends_in_the_block_before_it() {
    let pane = pane(&[FIRST, SECOND]);
    let (_home, host) = host();

    assert_eq!(under(&pane, PARAGRAPHS, [7, 7]), None, "the line alone");
    let seat = under(&pane, PARAGRAPHS, [0, 7]).unwrap();
    take(&host, &seat, Order::Last);

    assert_eq!(children(&pane), ["DIV.jp-text", "DIV.host", "DIV.jp-text"]);
}

/// `line one` is bytes 8 to 16 of a fenced block whose text runs to byte 36.
#[wasm_bindgen_test]
fn a_line_inside_a_code_block_seats_after_the_block_that_runs_past_it() {
    let html = concat!(
        r#"<pre><code class="language-rust"><span data-s="8" data-e="16">line one</span>"#,
        "\n",
        r#"<span data-s="17" data-e="25">line two</span>"#,
        "\n",
        r#"<span data-s="26" data-e="36">line three</span>"#,
        "\n</code></pre>",
    );
    let source = "```rust\nline one\nline two\nline three\n```\n\nNext\n";
    let next = r#"<p><span data-s="42" data-e="46">Next</span></p>"#;
    let pane = pane(&[html, next]);
    let (_home, host) = host();

    let seat = under(&pane, source, [0, 16]).unwrap();
    take(&host, &seat, Order::Last);

    assert_eq!(children(&pane), ["DIV.jp-text", "DIV.host", "DIV.jp-text"]);
    assert_eq!(end(&seat), Some(36), "past the line read, which ends at 16");
}

#[wasm_bindgen_test]
fn bytes_with_no_text_drawn_have_no_seat() {
    let pane = pane(&[FIRST, SECOND]);

    assert_eq!(under(&pane, PARAGRAPHS, [6, 8]), None);
}

/// Moving an element takes the keyboard from the field in it, which would
/// happen under the reader's hands each time the page looked the seat up.
#[wasm_bindgen_test]
fn sitting_where_it_sits_already_keeps_the_keyboard_in_its_field() {
    let pane = pane(&[LIST_HTML, FIRST]);
    let (_home, host) = host();
    let field: HtmlElement = host.first_element_child().unwrap().unchecked_into();
    let focused = || document().active_element() == Some(field.clone().into());

    for (bytes, order) in [
        ([2, 5], Order::First),
        ([18, 21], Order::First),
        ([2, 5], Order::Last),
        ([18, 21], Order::Last),
    ] {
        let seat = under(&pane, LIST, bytes).unwrap();
        take(&host, &seat, order);
        field.focus().unwrap();
        assert!(focused(), "the field takes the keyboard");

        take(&host, &under(&pane, LIST, bytes).unwrap(), order);
        assert!(focused(), "seated again at {bytes:?}, {order:?}");
    }
}

/// An element seated in a file's text carries the class that says it is no part
/// of the text, as the composer's and the read line's both do.
fn seated(name: &str) -> (Element, Element) {
    let (home, host) = host();
    host.set_class_name(&format!("jp-plain {name}"));

    (home, host)
}

/// The composer and the read line can sit under the same block: the composer
/// right under the text it writes about, whichever took the seat first.
/// Looking a seat up again moves neither, which would take the keyboard from
/// the composer.
#[wasm_bindgen_test]
fn two_elements_share_a_seat_each_in_its_place() {
    let pane = pane(&[FIRST, SECOND]);
    let (_home, composer) = seated("composer");
    let (_other, line) = seated("line");
    let field: HtmlElement = composer.first_element_child().unwrap().unchecked_into();
    let seat = || under(&pane, PARAGRAPHS, [0, 5]).unwrap();

    take(&line, &seat(), Order::Last);
    take(&composer, &seat(), Order::First);
    field.focus().unwrap();
    take(&line, &seat(), Order::Last);
    take(&composer, &seat(), Order::First);

    assert_eq!(children(&pane), [
        "DIV.jp-text",
        "DIV.jp-plain composer",
        "DIV.jp-plain line",
        "DIV.jp-text"
    ]);
    assert_eq!(document().active_element(), Some(field.into()));
}

#[wasm_bindgen_test]
fn two_elements_share_a_seat_in_a_list_item() {
    let pane = pane(&[LIST_HTML]);
    let (_home, composer) = seated("composer");
    let (_other, line) = seated("line");
    let seat = || under(&pane, LIST, [2, 5]).unwrap();

    take(&composer, &seat(), Order::First);
    take(&line, &seat(), Order::Last);
    take(&composer, &seat(), Order::First);

    let one = pane.query_selector(".one").unwrap().unwrap();
    assert_eq!(children(&one), [
        "SPAN.",
        "DIV.jp-plain composer",
        "DIV.jp-plain line",
        "UL."
    ]);
}

/// A list item's own text ends before the lists nested in it, and what is
/// seated in the text is none of it.
#[wasm_bindgen_test]
fn the_text_over_a_seat_ends_where_the_block_or_the_items_own_text_does() {
    let paragraphs = pane(&[FIRST, SECOND]);
    let list = pane(&[LIST_HTML]);
    let (_home, host) = seated("note");
    host.set_inner_html(r#"<span data-s="0" data-e="99">A comment.</span>"#);
    take(&host, &under(&list, LIST, [2, 5]).unwrap(), Order::First);

    assert_eq!(
        end(&under(&paragraphs, PARAGRAPHS, [0, 2]).unwrap()),
        Some(6)
    );
    assert_eq!(end(&under(&list, LIST, [2, 3]).unwrap()), Some(5));
    assert_eq!(end(&under(&list, LIST, [10, 12]).unwrap()), Some(15));
}

#[wasm_bindgen_test]
fn parking_takes_it_out_of_the_file() {
    let pane = pane(&[FIRST, SECOND]);
    let (home, host) = host();
    take(
        &host,
        &under(&pane, PARAGRAPHS, [0, 5]).unwrap(),
        Order::First,
    );

    park(&host, &home);

    assert_eq!(children(&pane), ["DIV.jp-text", "DIV.jp-text"]);
    assert_eq!(children(&home), ["DIV.host"]);
}
