use crate::format::{BackgroundFill, DefaultBackground, Formatter, TerminalOptions};

#[test]
fn reference_definitions_survive_beside_other_blocks() {
    let output = Formatter::with_width(0)
        .format_terminal("Before.\n\n[docs]: /docs\n\nAfter.\n")
        .unwrap();
    assert_eq!(output, "Before.\n\n[docs]: /docs\n\nAfter.\n");
}

#[test]
fn reference_definitions_survive_at_the_start_of_a_paragraph() {
    let output = Formatter::with_width(0)
        .format_terminal("[docs]: /docs\nRead **this**.\n")
        .unwrap();
    assert_eq!(output, "[docs]: /docs\n\nRead \x1b[1m**this**\x1b[22m.\n");
}

#[test]
fn reference_definitions_survive_inside_containers() {
    let output = Formatter::with_width(0)
        .format_terminal("- First.\n\n  [docs]: /docs\n\n  Last.\n")
        .unwrap();
    // The terminal writer retains the list prefix on blank lines.
    assert_eq!(output, "- First.\n  \n  [docs]: /docs\n  \n  Last.\n");

    let output = Formatter::with_width(0)
        .format_terminal("> [docs]: /docs\n")
        .unwrap();
    assert_eq!(output, "\x1b[38;2;131;148;150m> [docs]: /docs\x1b[39m\n");
}

#[test]
fn multiline_reference_definitions_preserve_their_source() {
    let output = Formatter::with_width(0)
        .format_terminal("[my\nlabel]:\n  <https://example.com/a_b>\n  \"A *literal* title\"\n")
        .unwrap();
    assert_eq!(
        output,
        "[my\nlabel]:\n  <https://example.com/a_b>\n  \"A *literal* title\"\n"
    );
}

#[test]
fn definitions_keep_duplicates_escaped_labels_and_relative_destinations() {
    let output = Formatter::with_width(0)
        .format_terminal("[a\\]b]: <../a_b> \"A *literal* title\"\n[a\\]b]: /second\n")
        .unwrap();
    assert_eq!(
        output,
        "[a\\]b]: <../a_b> \"A *literal* title\"\n[a\\]b]: /second\n"
    );
}

#[test]
fn resolved_links_and_unused_definitions_remain_visible() {
    let output = Formatter::with_width(0)
        .format_terminal("See [docs][id].\n\n[id]: /docs\n[unused]: /unused\n")
        .unwrap();
    assert_eq!(
        output,
        "See [docs](/docs).\n\n[id]: /docs\n[unused]: /unused\n"
    );
}

#[test]
fn definitions_in_nested_containers_keep_their_prefixes() {
    let output = Formatter::with_width(0)
        .format_terminal("- > [docs]: /docs\n  >\n  > Read **this**.\n")
        .unwrap();
    // Comrak classifies this as a tight list, so TerminalWriter suppresses
    // blank lines.
    assert_eq!(
        output,
        "- \x1b[38;2;131;148;150m> [docs]: /docs\n\x1b[38;2;131;148;150m  > Read \
         \x1b[1m**this**\x1b[22m.\x1b[39m\n"
    );
}

#[test]
fn definitions_inside_an_ordered_list_keep_continuation_indentation() {
    let output = Formatter::with_width(0)
        .format_terminal("10. [docs]: /docs\n    [other]: /other\n11. Read.\n")
        .unwrap();
    assert_eq!(
        output,
        "10. [docs]: /docs\n    [other]: /other\n11. Read.\n"
    );
}

#[test]
fn definitions_inside_a_task_item_remain_visible() {
    let output = Formatter::with_width(0)
        .format_terminal("- [x] Done.\n\n  [docs]: /docs\n")
        .unwrap();
    // Definitions do not make the parsed task list loose.
    assert_eq!(output, "- [x] Done.\n  [docs]: /docs\n");
}

#[test]
fn task_items_use_their_own_marker_width() {
    let output = Formatter::with_width(0)
        .format_terminal("9. [x] First.\n10. [x] Second.\n\n    [docs]: /docs\n")
        .unwrap();
    assert_eq!(
        output,
        "9. [x] First.\n10. [x] Second.\n    [docs]: /docs\n"
    );
}

#[test]
fn definitions_before_a_setext_heading_remain_visible() {
    let output = Formatter::with_width(0)
        .format_terminal("[docs]: /docs\nTitle\n=====\n")
        .unwrap();
    assert_eq!(output, "[docs]: /docs\n\n# \x1b[1mTitle\x1b[22m\n");
}

#[test]
fn definition_lookalikes_inside_code_and_html_are_not_duplicated() {
    let formatter = Formatter::with_width(0);
    assert_eq!(
        formatter
            .format_terminal("```\n[docs]: /docs\n```\n")
            .unwrap(),
        "```\n[docs]: /docs\n```\n"
    );
    assert_eq!(
        formatter.format_terminal("    [docs]: /docs\n").unwrap(),
        "```\n[docs]: /docs\n```\n"
    );
    assert_eq!(
        formatter
            .format_terminal("<pre>\n[docs]: /docs\n</pre>\n")
            .unwrap(),
        "<pre>\n[docs]: /docs\n</pre>\n"
    );
}

#[test]
fn invalid_definitions_and_definitions_after_prose_remain_ordinary_markdown() {
    let formatter = Formatter::with_width(0);
    assert_eq!(
        formatter
            .format_terminal("[docs]: /docs trailing **text**\n")
            .unwrap(),
        "[docs]: /docs trailing \x1b[1m**text**\x1b[22m\n"
    );
    assert_eq!(
        formatter.format_terminal("Read.\n[docs]: /docs\n").unwrap(),
        "Read.\n[docs]: /docs\n"
    );
    assert_eq!(
        formatter.format_terminal("\\[docs]: /docs\n").unwrap(),
        "[docs]: /docs\n"
    );
}

#[test]
fn definition_line_endings_are_normalized() {
    let formatter = Formatter::with_width(0);
    assert_eq!(
        formatter
            .format_terminal("[docs]:\r\n  /docs\r\n  \"Title\"\r\n")
            .unwrap(),
        "[docs]:\n  /docs\n  \"Title\"\n"
    );
    assert_eq!(
        formatter
            .format_terminal("[docs]: /docs\r\rAfter.\r")
            .unwrap(),
        "[docs]: /docs\n\nAfter.\n"
    );
}

#[test]
fn definitions_use_the_terminal_indent_wrap_width_and_background() {
    let output = Formatter::with_width(22)
        .format_terminal_with("[docs]: /docs \"A title with words\"\n", &TerminalOptions {
            indent: 2,
            default_background: Some(DefaultBackground {
                param: "48;5;236".into(),
                fill: BackgroundFill::Content,
            }),
            suppress_trailing_separator: true,
            ..TerminalOptions::default()
        })
        .unwrap();
    assert_eq!(
        output,
        "\x1b[48;5;236m  [docs]: /docs \"A\x1b[0m\n\x1b[48;5;236m  title with words\"\x1b[0m\n"
    );
}

#[test]
fn definitions_in_nested_lists_and_tab_indented_items_keep_their_destinations() {
    let formatter = Formatter::with_width(0);
    assert_eq!(
        formatter
            .format_terminal(" - [outer]: /outer\n   - [inner]: /inner\n")
            .unwrap(),
        "- [outer]: /outer\n  - [inner]: /inner\n"
    );
    assert_eq!(
        formatter
            .format_terminal("-\t[docs]: /docs\n\t[other]: /other\n")
            .unwrap(),
        "- [docs]: /docs\n  [other]: /other\n"
    );
}

#[test]
fn nested_list_tabs_are_measured_from_their_source_column() {
    let output = Formatter::with_width(0)
        .format_terminal("- - [docs]: /docs\n  \t   [other]: /other\n    Read.\n")
        .unwrap();
    assert_eq!(
        output,
        "- - [docs]: /docs\n       [other]: /other\n    Read.\n"
    );
}

#[test]
fn definitions_separated_by_blank_lines_are_preserved_once() {
    let output = Formatter::with_width(0)
        .format_terminal("\n[one]: /one\n\n[two]: /two\n\nAfter.\n")
        .unwrap();
    assert_eq!(output, "[one]: /one\n\n[two]: /two\n\nAfter.\n");
}

#[test]
fn definitions_with_unicode_labels_keep_their_source() {
    let output = Formatter::with_width(0)
        .format_terminal("[日本語]: /docs \"Français\"\n")
        .unwrap();
    assert_eq!(output, "[日本語]: /docs \"Français\"\n");
}

#[test]
fn commonmark_serialization_still_resolves_definitions() {
    let output = Formatter::with_width(0)
        .format("See [docs][id].\n\n[id]: /docs\n[unused]: /unused\n")
        .unwrap();
    assert_eq!(output, "See [docs](/docs).\n");
}
