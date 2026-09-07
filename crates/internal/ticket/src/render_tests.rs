use indoc::indoc;

use super::*;
use crate::{Kind, Labels, Vocabulary, parse};

/// A ticket with everything but the parts under test fixed.
fn draft<'a>(
    title: &'a str,
    kind: Kind,
    authors: &'a str,
    labels: &'a Labels,
    description: &'a str,
) -> NewTicket<'a> {
    NewTicket {
        kind,
        title,
        authors,
        date: "2026-08-05",
        implements: None,
        labels,
        description,
    }
}

fn new_comment(from: &str, body: &str, re: Option<&str>) -> Comment {
    Comment {
        from: from.to_owned(),
        date: "2026-08-05T14:03:11Z".to_owned(),
        re: re.map(ToOwned::to_owned),
        body: body.to_owned(),
    }
}

#[test]
fn renders_a_new_ticket() {
    let out = ticket(&draft(
        "Tool call header misaligned",
        Kind::Bug,
        "John Doe",
        &Labels::default(),
        "The header renders one column left of the body.",
    ));

    assert_eq!(out, indoc! {"
            # Tool call header misaligned

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: John Doe
            - **Date**: 2026-08-05

            The header renders one column left of the body.
        "});
}

#[test]
fn renders_a_new_ticket_without_a_description() {
    let out = ticket(&draft(
        "Bump the deny list",
        Kind::Chore,
        "john",
        &Labels::default(),
        "   ",
    ));

    assert_eq!(out, indoc! {"
            # Bump the deny list

            - **Status**: Todo
            - **Kind**: Chore
            - **Authors**: john
            - **Date**: 2026-08-05
        "});
}

#[test]
fn first_comment_opens_the_comments_section() {
    let document = ticket(&draft(
        "Tool call header misaligned",
        Kind::Bug,
        "John Doe",
        &Labels::default(),
        "The header renders one column left of the body.",
    ));

    let out = append_comment(
        &document,
        &new_comment("john", "Reproduced at 72 columns.", None),
    );

    assert_eq!(out, indoc! {"
            # Tool call header misaligned

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: John Doe
            - **Date**: 2026-08-05

            The header renders one column left of the body.

            ## Comments

            -----

            - **From**: john
            - **Date**: 2026-08-05T14:03:11Z

            Reproduced at 72 columns.
        "});
}

#[test]
fn renders_a_comment_block() {
    let out = comment(&new_comment(
        "jp",
        "The wrap calculation is off.",
        Some("#1"),
    ));

    assert_eq!(out, indoc! {"
            -----

            - **From**: jp
            - **Date**: 2026-08-05T14:03:11Z
            - **Re**: #1

            The wrap calculation is off.
        "});
}

/// A second comment is written at the end and nothing above it moves.
#[test]
fn later_comments_are_a_pure_append() {
    let document = append_comment(
        &ticket(&draft(
            "Tool call header misaligned",
            Kind::Bug,
            "John Doe",
            &Labels::default(),
            "Description.",
        )),
        &new_comment("john", "Reproduced at 72 columns.", None),
    );

    let out = append_comment(
        &document,
        &new_comment("jp", "The wrap calculation is off.", Some("#1")),
    );

    assert!(out.starts_with(&document));
    assert_eq!(out.strip_prefix(&document).unwrap(), indoc! {"

            -----

            - **From**: jp
            - **Date**: 2026-08-05T14:03:11Z
            - **Re**: #1

            The wrap calculation is off.
        "});
}

#[test]
fn appended_comments_parse_back() {
    let document = append_comment(
        &append_comment(
            &ticket(&draft(
                "Round trip",
                Kind::Feature,
                "john",
                &Labels::default(),
                "Description.",
            )),
            &new_comment("john", "First.", None),
        ),
        &new_comment("jp", "Second.", Some("#1")),
    );

    let parsed = parse::document(&document).unwrap();

    assert_eq!(parsed.description, "Description.");
    assert_eq!(parsed.comments.len(), 2);
    assert_eq!(parsed.comments[0].body, "First.");
    assert_eq!(parsed.comments[1].body, "Second.");
    assert_eq!(parsed.comments[1].re.as_deref(), Some("#1"));
}

#[test]
fn replaces_a_metadata_field() {
    let document = indoc! {"
        # Tool call header misaligned

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: John Doe
        - **Date**: 2026-08-05

        Description.
    "};

    let out = set_metadata(document, "Status", "Done").unwrap();

    assert!(out.contains("- **Status**: Done\n"));
    assert!(!out.contains("- **Status**: Todo"));
    assert!(out.ends_with("Description.\n"));
}

/// A quoted metadata line in a comment is content, not the ticket's own state.
#[test]
fn only_the_header_block_is_rewritten() {
    let document = indoc! {"
        # Quoting metadata

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: john
        - **Date**: 2026-08-05

        ## Comments

        -----

        - **From**: jp
        - **Date**: 2026-08-05T14:31:02Z

        The header should read:

        - **Status**: Todo
    "};

    let out = set_metadata(document, "Status", "Done").unwrap();

    assert_eq!(out.matches("- **Status**: Todo").count(), 1);
    assert_eq!(out.matches("- **Status**: Done").count(), 1);
}

/// A field the ticket doesn't carry yet joins the end of its block, which is
/// how an import records the issue it came from.
#[test]
fn adds_a_field_the_ticket_lacks() {
    let document = indoc! {"
        # Tool call header misaligned

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: john
        - **Date**: 2026-08-05

        Description.
    "};

    let out = set_metadata(document, "GitHub", "#123").unwrap();

    assert_eq!(out, indoc! {"
            # Tool call header misaligned

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: john
            - **Date**: 2026-08-05
            - **GitHub**: #123

            Description.
        "});
}

#[test]
fn reports_a_document_with_no_metadata_block() {
    assert_eq!(set_metadata("# Bare\n\nProse.\n", "Status", "Done"), None);
    assert_eq!(remove_metadata("# Bare\n\nProse.\n", "Label"), None);
}

#[test]
fn removing_a_repeated_field_drops_every_line() {
    let document = indoc! {"
        # Tool call header misaligned

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: john
        - **Date**: 2026-08-05
        - **Label**: client=cli
        - **Label**: package=jp_cli

        Description.
    "};

    let out = remove_metadata(document, "Label").unwrap();

    assert_eq!(out, indoc! {"
            # Tool call header misaligned

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: john
            - **Date**: 2026-08-05

            Description.
        "});
}

/// Clearing labels on a ticket that has none is not an error, so a caller
/// doesn't have to read the ticket first to know which write to make.
#[test]
fn removing_a_field_the_ticket_lacks_changes_nothing() {
    let document = indoc! {"
        # Tool call header misaligned

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: john
        - **Date**: 2026-08-05

        Description.
    "};

    assert_eq!(remove_metadata(document, "Label").unwrap(), document);
}

/// The replacement lands where the old run was, so relabelling doesn't shuffle
/// the block.
#[test]
fn replaces_a_repeated_field_in_place() {
    let document = indoc! {"
        # Labelled

        - **Status**: Todo
        - **Label**: client=cli
        - **Label**: package=jp_cli
        - **Authors**: john

        Description.
    "};

    let out = set_repeated_metadata(document, "Label", &["client=web".to_owned()]).unwrap();

    assert_eq!(out, indoc! {"
            # Labelled

            - **Status**: Todo
            - **Label**: client=web
            - **Authors**: john

            Description.
        "});
}

/// One line per pair, so the block survives a formatter that wraps at a fixed
/// width no matter how many labels a ticket carries.
#[test]
fn renders_one_line_per_label() {
    let vocabulary = Vocabulary::parse(
        r#"{"package": {"values": ["jp_cli", "jp_config"]}, "client": {"values": ["cli"]}}"#,
    )
    .unwrap();
    let labels = vocabulary
        .resolve(&[
            "package=jp_config".to_owned(),
            "client=cli".to_owned(),
            "package=jp_cli".to_owned(),
        ])
        .unwrap();

    let out = ticket(&draft(
        "Labelled",
        Kind::Bug,
        "john",
        &labels,
        "Description.",
    ));

    assert_eq!(out, indoc! {"
            # Labelled

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: john
            - **Date**: 2026-08-05
            - **Label**: client=cli
            - **Label**: package=jp_cli
            - **Label**: package=jp_config

            Description.
        "});
}
