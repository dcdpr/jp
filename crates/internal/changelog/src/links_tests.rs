use indoc::indoc;

use super::*;

fn standalone(markdown: &str) -> String {
    Links::read(markdown)
        .standalone(markdown, 0..markdown.len())
        .trim_end()
        .to_owned()
}

#[test]
fn a_full_reference_takes_the_url_its_label_defines() {
    assert_eq!(
        standalone("See [the docs][docs].\n\n[docs]: https://example.com/docs\n"),
        "See [the docs](https://example.com/docs)."
    );
}

#[test]
fn collapsed_and_shortcut_references_keep_their_text() {
    assert_eq!(
        standalone(indoc! {"
            See [docs][] and [docs].

            [docs]: https://example.com/docs
        "}),
        "See [docs](https://example.com/docs) and [docs](https://example.com/docs)."
    );
}

#[test]
fn a_definitions_title_comes_along() {
    assert_eq!(
        standalone(indoc! {r#"
            See [the docs].

            [the docs]: https://example.com/docs "The \"full\" docs"
        "#}),
        r#"See [the docs](https://example.com/docs "The \"full\" docs")."#
    );
}

#[test]
fn a_reference_image_is_rewritten_too() {
    assert_eq!(
        standalone("![The logo][logo]\n\n[logo]: https://example.com/logo.png\n"),
        "![The logo](https://example.com/logo.png)"
    );
}

#[test]
fn an_image_in_a_links_text_is_rewritten_with_it() {
    assert_eq!(
        standalone(indoc! {"
            [![The logo][logo]][home]

            [home]: https://example.com
            [logo]: https://example.com/logo.png
        "}),
        "[![The logo](https://example.com/logo.png)](https://example.com)"
    );
}

/// A code span binds tighter than a link's brackets, so its `]` doesn't close
/// the text.
#[test]
fn a_code_span_in_a_links_text_keeps_its_brackets() {
    assert_eq!(
        standalone("[`a]b`][ref]\n\n[ref]: https://example.com\n"),
        "[`a]b`](https://example.com)"
    );
}

#[test]
fn inline_links_and_autolinks_are_left_as_written() {
    let markdown = "See [the docs](https://example.com/docs) or <https://example.com>.";

    assert_eq!(standalone(markdown), markdown);
}

/// With no definition, `[not a link]` is text.
#[test]
fn a_label_without_a_definition_is_left_as_written() {
    assert_eq!(
        standalone("Leave [not a link] alone."),
        "Leave [not a link] alone."
    );
}

#[test]
fn a_definition_in_a_code_block_is_code() {
    let markdown = indoc! {"
        An example:

        ```text
        [docs]: https://example.com/docs
        ```
    "};

    assert_eq!(standalone(markdown), markdown.trim_end());
}

/// Once every link is inline nothing reads a definition, so it goes even when
/// no link used it.
#[test]
fn every_definition_goes_with_its_line_break() {
    let markdown = indoc! {"
        Write `[docs]` for a link.

        [docs]: https://example.com/docs
        [unused]: https://example.com/unused
    "};

    assert_eq!(
        Links::read(markdown).standalone(markdown, 0..markdown.len()),
        "Write `[docs]` for a link.\n\n"
    );
}

#[test]
fn a_part_takes_its_urls_from_definitions_outside_it() {
    let markdown = indoc! {"
        See [a].

        Then [b].

        [a]: https://example.com/a
        [b]: https://example.com/b
    "};
    let links = Links::read(markdown);
    let second = markdown.find("Then").unwrap();

    assert_eq!(
        links.standalone(markdown, 0..second),
        "See [a](https://example.com/a).\n\n"
    );
    assert_eq!(
        links.standalone(markdown, second..markdown.len()),
        "Then [b](https://example.com/b).\n\n"
    );
}

#[test]
fn a_destination_that_cant_stand_bare_goes_in_angle_brackets() {
    assert_eq!(
        inline("https://en.wikipedia.org/wiki/Rust_(language)", ""),
        "(https://en.wikipedia.org/wiki/Rust_(language))"
    );
    assert_eq!(
        inline("https://example.com/a b", ""),
        "(<https://example.com/a b>)"
    );
    assert_eq!(
        inline("https://example.com/a)b", ""),
        "(<https://example.com/a)b>)"
    );
    assert_eq!(inline(r"C:\docs<1>", ""), r"(<C:\\docs\<1\>>)");
    assert_eq!(inline("", "Nothing yet"), r#"(<> "Nothing yet")"#);
}

#[test]
fn a_title_stays_on_one_line() {
    assert_eq!(
        inline("https://example.com", "Two\nlines"),
        r#"(https://example.com "Two lines")"#
    );
}
