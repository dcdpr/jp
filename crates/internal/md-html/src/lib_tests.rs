use std::collections::BTreeMap;

use indoc::indoc;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn markdown_syntax_between_text_is_in_no_span() {
    let html = render("This RFD is **Abandoned**.\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="12">This RFD is </span><strong><span data-s="14" data-e="23">Abandoned</span></strong><span data-s="25" data-e="26">.</span></p>
    "#});
}

#[test]
fn offsets_count_utf8_bytes() {
    let html = render("Olá, **世界**!\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="6">Olá, </span><strong><span data-s="8" data-e="14">世界</span></strong><span data-s="16" data-e="17">!</span></p>
    "#});
}

#[test]
fn line_ending_between_lines_of_a_paragraph_is_in_no_span() {
    let html = render("one\ntwo\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="3">one</span>
        <span data-s="4" data-e="7">two</span></p>
    "#});
}

#[test]
fn crlf_line_endings_are_in_no_span() {
    let html = render("one\r\ntwo\r\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="3">one</span>
        <span data-s="5" data-e="8">two</span></p>
    "#});
}

#[test]
fn backslash_of_an_escape_is_in_no_span() {
    let html = render("a \\* b\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="2">a </span><span data-s="3" data-e="4">*</span><span data-s="4" data-e="6"> b</span></p>
    "#});
}

#[test]
fn heading_text_is_mapped() {
    let html = render("## Title\n").html;

    assert_eq!(html, indoc! {r#"
        <h2><span data-s="3" data-e="8">Title</span></h2>
    "#});
}

/// The second heading starts on line 2, counted from 0, and no anchor names the
/// line of the third.
#[test]
fn a_heading_carries_the_anchor_named_for_its_line() {
    let anchors = BTreeMap::from([(0, "title".to_owned()), (2, "a-b".to_owned())]);

    let html = render_with_anchors("# Title\n\n## A & B\n\n### Bare\n", &anchors).html;

    assert_eq!(html, indoc! {r#"
        <h1 id="title"><span data-s="2" data-e="7">Title</span></h1>
        <h2 id="a-b"><span data-s="12" data-e="17">A &amp; B</span></h2>
        <h3><span data-s="23" data-e="27">Bare</span></h3>
    "#});
}

#[test]
fn an_underlined_heading_carries_the_anchor_of_the_line_its_text_is_on() {
    let anchors = BTreeMap::from([(2, "title".to_owned())]);

    let html = render_with_anchors("Lead.\n\nTitle\n=====\n", &anchors).html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="5">Lead.</span></p>
        <h1 id="title"><span data-s="7" data-e="12">Title</span></h1>
    "#});
}

#[test]
fn an_anchor_is_escaped_as_an_attribute() {
    let anchors = BTreeMap::from([(0, "a\"b<c".to_owned())]);

    let html = render_with_anchors("# T\n", &anchors).html;

    assert_eq!(
        html,
        "<h1 id=\"a&quot;b&lt;c\"><span data-s=\"2\" data-e=\"3\">T</span></h1>\n"
    );
}

#[test]
fn link_text_is_mapped_and_its_destination_is_not() {
    let html = render("[docs](https://example.com)\n").html;

    assert_eq!(html, indoc! {r#"
        <p><a href="https://example.com"><span data-s="1" data-e="5">docs</span></a></p>
    "#});
}

#[test]
fn javascript_link_destination_is_dropped() {
    let html = render("[x](javascript:alert(1))\n").html;

    assert_eq!(html, indoc! {r#"
        <p><a href=""><span data-s="1" data-e="2">x</span></a></p>
    "#});
}

#[test]
fn autolinked_url_is_mapped() {
    let html = render("See https://example.com now.\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="4">See </span><a href="https://example.com"><span data-s="4" data-e="23">https://example.com</span></a><span data-s="23" data-e="28"> now.</span></p>
    "#});
}

#[test]
fn image_alt_text_is_an_attribute_without_a_span() {
    let html = render("![alt text](a.png)\n").html;

    assert_eq!(html, indoc! {r#"
        <p><img src="a.png" alt="alt text" /></p>
    "#});
}

#[test]
fn task_list_item_text_is_mapped() {
    let html = render("- [x] done\n").html;

    assert_eq!(html, indoc! {r#"
        <ul>
        <li><input type="checkbox" checked="" disabled="" /> <span data-s="6" data-e="10">done</span></li>
        </ul>
    "#});
}

#[test]
fn tab_inside_text_is_mapped() {
    let html = render("a\tb\n").html;

    assert_eq!(html, "<p><span data-s=\"0\" data-e=\"3\">a\tb</span></p>\n");
}

#[test]
fn code_span_backticks_are_in_no_span() {
    let html = render("Run `jp query` now.\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="4">Run </span><code><span data-s="5" data-e="13">jp query</span></code><span data-s="14" data-e="19"> now.</span></p>
    "#});
}

#[test]
fn wrapped_code_span_maps_each_source_line() {
    let html = render("A `code\nspan` here.\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="2">A </span><code><span data-s="3" data-e="7">code</span> <span data-s="8" data-e="12">span</span></code><span data-s="13" data-e="19"> here.</span></p>
    "#});
}

#[test]
fn wrapped_code_span_with_crlf_line_endings_maps_each_source_line() {
    let html = render("A `code\r\nspan` here.\r\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="2">A </span><code><span data-s="3" data-e="7">code</span> <span data-s="9" data-e="13">span</span></code><span data-s="14" data-e="20"> here.</span></p>
    "#});
}

#[test]
fn wrapped_code_span_in_a_list_item_skips_the_indentation() {
    let html = render("- a `code\n  span` b\n").html;

    assert_eq!(html, indoc! {r#"
        <ul>
        <li><span data-s="2" data-e="4">a </span><code><span data-s="5" data-e="9">code</span> <span data-s="12" data-e="16">span</span></code><span data-s="17" data-e="19"> b</span></li>
        </ul>
    "#});
}

#[test]
fn wrapped_code_span_in_a_block_quote_skips_the_marker() {
    let html = render("> `code\n> span`\n").html;

    assert_eq!(html, indoc! {r#"
        <blockquote>
        <p><code><span data-s="3" data-e="7">code</span> <span data-s="10" data-e="14">span</span></code></p>
        </blockquote>
    "#});
}

#[test]
fn code_span_padding_is_in_no_span() {
    let html = render("a `` `x` `` b\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="2">a </span><code><span data-s="5" data-e="8">`x`</span></code><span data-s="11" data-e="13"> b</span></p>
    "#});
}

#[test]
fn code_span_padded_by_line_endings_maps_only_its_middle_line() {
    let html = render("x ``\nfoo\n`` y\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="2">x </span><code><span data-s="5" data-e="8">foo</span></code><span data-s="11" data-e="13"> y</span></p>
    "#});
}

#[test]
fn fenced_code_block_maps_each_line() {
    let html = render("```rust\nfn main() {}\n\nlet x = 1;\n```\n").html;

    assert_eq!(html, indoc! {r#"
        <pre><code class="language-rust"><span data-s="8" data-e="20">fn main() {}</span>

        <span data-s="22" data-e="32">let x = 1;</span>
        </code></pre>
    "#});
}

#[test]
fn code_block_in_a_block_quote_maps_after_the_marker() {
    let html = render("> ```\n> a\n> ```\n").html;

    assert_eq!(html, indoc! {r#"
        <blockquote>
        <pre><code><span data-s="8" data-e="9">a</span>
        </code></pre>
        </blockquote>
    "#});
}

#[test]
fn indented_code_block_maps_after_the_indentation() {
    let html = render("    code\n").html;

    assert_eq!(html, indoc! {r#"
        <pre><code><span data-s="4" data-e="8">code</span>
        </code></pre>
    "#});
}

#[test]
fn tab_indented_code_block_maps_after_the_tab() {
    let html = render("\tcode\n").html;

    assert_eq!(html, indoc! {r#"
        <pre><code><span data-s="1" data-e="5">code</span>
        </code></pre>
    "#});
}

#[test]
fn code_block_with_crlf_line_endings_maps_each_line() {
    let html = render("```\r\na\r\n```\r\n").html;

    assert_eq!(
        html,
        "<pre><code><span data-s=\"5\" data-e=\"6\">a</span>\r\n</code></pre>\n"
    );
}

#[test]
fn escaped_pipe_in_a_table_cell_is_an_atom() {
    let html = render("| a | b |\n| - | - |\n| x \\| y | z |\n").html;

    assert_eq!(html, indoc! {r#"
        <table>
        <thead>
        <tr>
        <th><span data-s="2" data-e="3">a</span></th>
        <th><span data-s="6" data-e="7">b</span></th>
        </tr>
        </thead>
        <tbody>
        <tr>
        <td><span data-s="22" data-e="24">x </span><span data-s="24" data-e="26" data-atom>|</span><span data-s="26" data-e="28"> y</span></td>
        <td><span data-s="31" data-e="32">z</span></td>
        </tr>
        </tbody>
        </table>
    "#});
}

#[test]
fn escaped_pipe_in_a_code_span_in_a_table_cell_is_an_atom() {
    let html = render("| a |\n| - |\n| `x\\|y` |\n").html;

    assert_eq!(html, indoc! {r#"
        <table>
        <thead>
        <tr>
        <th><span data-s="2" data-e="3">a</span></th>
        </tr>
        </thead>
        <tbody>
        <tr>
        <td><code><span data-s="15" data-e="16">x</span><span data-s="16" data-e="18" data-atom>|</span><span data-s="18" data-e="19">y</span></code></td>
        </tr>
        </tbody>
        </table>
    "#});
}

#[test]
fn alert_title_is_an_atom_for_its_marker() {
    let html = render("> [!NOTE]\n> Read this.\n").html;

    assert_eq!(html, indoc! {r#"
        <div class="markdown-alert markdown-alert-note">
        <p class="markdown-alert-title"><span data-s="2" data-e="9" data-atom>Note</span></p>
        <p><span data-s="12" data-e="22">Read this.</span></p>
        </div>
    "#});
}

#[test]
fn alert_with_its_own_title_maps_the_title() {
    let html = render("> [!TIP] Heads up\n> Body.\n").html;

    assert_eq!(html, indoc! {r#"
        <div class="markdown-alert markdown-alert-tip">
        <p class="markdown-alert-title"><span data-s="9" data-e="17">Heads up</span></p>
        <p><span data-s="20" data-e="25">Body.</span></p>
        </div>
    "#});
}

#[test]
fn inline_html_is_escaped_text() {
    let html = render("a <b>bold</b> c\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span data-s="0" data-e="2">a </span><span data-s="2" data-e="5">&lt;b&gt;</span><span data-s="5" data-e="9">bold</span><span data-s="9" data-e="13">&lt;/b&gt;</span><span data-s="13" data-e="15"> c</span></p>
    "#});
}

#[test]
fn html_block_is_escaped_preformatted_text() {
    let html = render("<!--\nnote\n-->\n").html;

    assert_eq!(html, indoc! {r#"
        <pre><span data-s="0" data-e="4">&lt;!--</span>
        <span data-s="5" data-e="9">note</span>
        <span data-s="10" data-e="13">--&gt;</span>
        </pre>
    "#});
}

#[test]
fn text_with_an_entity_is_unmapped() {
    let html = render("Tom &amp; Jerry\n").html;

    assert_eq!(html, indoc! {r#"
        <p><span class="unmapped">Tom &amp; Jerry</span></p>
    "#});
}

#[test]
fn text_with_a_nul_byte_is_unmapped() {
    // comrak replaces NUL (1 byte) with U+FFFD (3 bytes), shifting subsequent offsets.
    let html = render("a\0b\n").html;

    assert_eq!(html, "<p><span class=\"unmapped\">a\u{fffd}b</span></p>\n");
}
