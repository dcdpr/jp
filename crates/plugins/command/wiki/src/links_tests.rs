use indoc::indoc;

use super::*;

/// A page the way `comfort` leaves one: inline links moved to reference style,
/// definitions at the bottom, images kept inline.
const PAGE: &str = indoc! {"
    # Links

    See [`Links::by_rfd`] and [the parser][parser], or [RFD 100](../rfd/100-x.md#labels).

    ![Pinned Conversation](0a1b2c3-ubiquitous-language.md#pinned-conversation)

    Text and ![inline](x.png) image.

    ```markdown
    [not](a-link.md)
    ```

    [`Links::by_rfd`]: ../../crates/plugins/command/rfd/src/links.rs#L88-L125
    [parser]: ../../crates/jp_md/src/lib.rs
"};

#[test]
fn reads_every_link_with_its_destination_resolved() {
    let found = links(PAGE);
    let urls: Vec<(&str, bool, bool)> = found
        .iter()
        .map(|link| (link.url.as_str(), link.image, link.inline))
        .collect();

    assert_eq!(urls, [
        (
            "../../crates/plugins/command/rfd/src/links.rs#L88-L125",
            false,
            false
        ),
        ("../../crates/jp_md/src/lib.rs", false, false),
        ("../rfd/100-x.md#labels", false, true),
        (
            "0a1b2c3-ubiquitous-language.md#pinned-conversation",
            true,
            true
        ),
        ("x.png", true, true),
    ]);
}

#[test]
fn a_code_span_label_is_kept() {
    let found = links(PAGE);

    assert_eq!(found[0].code_label.as_deref(), Some("Links::by_rfd"));
    assert_eq!(found[1].code_label, None);
}

#[test]
fn an_image_alone_in_its_paragraph_is_standalone() {
    let found = links(PAGE);

    assert_eq!(found[3].standalone, Some(4..5));
    assert_eq!(found[4].standalone, None);
}

#[test]
fn spans_cover_the_link_and_its_text() {
    let found = links(PAGE);
    let link = &found[2];

    assert_eq!(
        &PAGE[link.span.clone()],
        "[RFD 100](../rfd/100-x.md#labels)"
    );
    assert_eq!(&PAGE[link.text.clone().unwrap()], "RFD 100");
    assert_eq!(link.line, 2);
}

#[test]
fn mentions_are_read_outside_code_blocks() {
    let source = indoc! {"
        # Mentions

        See W-0a1b2c3#pinned-conversation, `T-02wt0kx`, and RFD
        100#labels. Not W-0A1B2C3 or xW-0a1b2c3.

        ```text
        W-0b0b0b0
        ```
    "};

    let found = mentions(source);

    assert_eq!(found, [
        Mention {
            named: Named::Wiki,
            id: "W-0a1b2c3".to_owned(),
            fragment: Some("pinned-conversation".to_owned()),
            line: 2,
        },
        Mention {
            named: Named::Ticket,
            id: "T-02wt0kx".to_owned(),
            fragment: None,
            line: 2,
        },
        Mention {
            named: Named::Rfd,
            id: "100".to_owned(),
            fragment: Some("labels".to_owned()),
            line: 2,
        },
    ]);
}

#[test]
fn line_ranges_read_every_form() {
    assert_eq!(line_range("L12"), Some((12, 12)));
    assert_eq!(line_range("L12-L24"), Some((12, 24)));
    assert_eq!(line_range("L12-24"), Some((12, 24)));
    assert_eq!(line_range("12-24"), Some((12, 24)));
    assert_eq!(line_range("pinned-conversation"), None);
    assert_eq!(line_range("12"), None);
    assert_eq!(line_range("L"), None);
}

#[test]
fn a_symbol_is_the_trailing_identifier() {
    assert_eq!(symbol("Links::by_rfd"), Some("by_rfd"));
    assert_eq!(symbol("by_rfd()"), Some("by_rfd"));
    assert_eq!(symbol("::"), None);
}

#[test]
fn destinations_resolve_against_the_document() {
    let dir = Utf8Path::new("/w/docs/wiki");

    assert_eq!(
        resolve(dir, "../../crates/a%20b.rs"),
        Utf8PathBuf::from("/w/crates/a b.rs")
    );
    assert_eq!(
        resolve(dir, "./x.md"),
        Utf8PathBuf::from("/w/docs/wiki/x.md")
    );
    assert!(has_scheme("https://example.com"));
    assert!(has_scheme("mailto:a@b.c"));
    assert!(!has_scheme("C:/x"));
    assert!(!has_scheme("../x.md"));
    assert_eq!(split_fragment("a.md#b"), ("a.md", Some("b")));
}
