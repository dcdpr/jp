use indoc::indoc;

use super::*;
use crate::parse;

fn comment(from: &str, body: &str, re: Option<&str>) -> Comment {
    Comment {
        from: from.to_owned(),
        date: "2026-08-05T14:03:11Z".to_owned(),
        re: re.map(ToOwned::to_owned),
        body: body.to_owned(),
    }
}

#[test]
fn renders_a_new_ticket() {
    let out = ticket(
        TicketId::new(42),
        "Tool call header misaligned",
        Kind::Bug,
        "Jean Mertz",
        "2026-08-05",
        None,
        "The header renders one column left of the body.",
    );

    assert_eq!(out, indoc! {"
            # T0042: Tool call header misaligned

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: Jean Mertz
            - **Date**: 2026-08-05

            The header renders one column left of the body.
        "});
}

#[test]
fn renders_a_new_ticket_without_a_description() {
    let out = ticket(
        TicketId::new(1),
        "Bump the deny list",
        Kind::Chore,
        "jean",
        "2026-08-05",
        None,
        "   ",
    );

    assert_eq!(out, indoc! {"
            # T0001: Bump the deny list

            - **Status**: Todo
            - **Kind**: Chore
            - **Authors**: jean
            - **Date**: 2026-08-05
        "});
}

#[test]
fn first_comment_opens_the_comments_section() {
    let document = ticket(
        TicketId::new(42),
        "Tool call header misaligned",
        Kind::Bug,
        "Jean Mertz",
        "2026-08-05",
        None,
        "The header renders one column left of the body.",
    );

    let out = append_comment(
        &document,
        &comment("jean", "Reproduced at 72 columns.", None),
    );

    assert_eq!(out, indoc! {"
            # T0042: Tool call header misaligned

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: Jean Mertz
            - **Date**: 2026-08-05

            The header renders one column left of the body.

            ## Comments

            -----

            - **From**: jean
            - **Date**: 2026-08-05T14:03:11Z

            Reproduced at 72 columns.
        "});
}

/// A second comment is written at the end and nothing above it moves.
#[test]
fn later_comments_are_a_pure_append() {
    let document = append_comment(
        &ticket(
            TicketId::new(42),
            "Tool call header misaligned",
            Kind::Bug,
            "Jean Mertz",
            "2026-08-05",
            None,
            "Description.",
        ),
        &comment("jean", "Reproduced at 72 columns.", None),
    );

    let out = append_comment(
        &document,
        &comment("jp", "The wrap calculation is off.", Some("T0042#1")),
    );

    assert!(out.starts_with(&document));
    assert_eq!(out.strip_prefix(&document).unwrap(), indoc! {"

            -----

            - **From**: jp
            - **Date**: 2026-08-05T14:03:11Z
            - **Re**: T0042#1

            The wrap calculation is off.
        "});
}

#[test]
fn appended_comments_parse_back() {
    let document = append_comment(
        &append_comment(
            &ticket(
                TicketId::new(7),
                "Round trip",
                Kind::Feature,
                "jean",
                "2026-08-05",
                None,
                "Description.",
            ),
            &comment("jean", "First.", None),
        ),
        &comment("jp", "Second.", Some("T0007#1")),
    );

    let parsed = parse::document(&document).unwrap();

    assert_eq!(parsed.description, "Description.");
    assert_eq!(parsed.comments.len(), 2);
    assert_eq!(parsed.comments[0].body, "First.");
    assert_eq!(parsed.comments[1].body, "Second.");
    assert_eq!(parsed.comments[1].re.as_deref(), Some("T0007#1"));
}

#[test]
fn replaces_a_metadata_field() {
    let document = indoc! {"
        # T0042: Tool call header misaligned

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: Jean Mertz
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
        # T0042: Quoting metadata

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: jean
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
        # T0042: Tool call header misaligned

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: jean
        - **Date**: 2026-08-05

        Description.
    "};

    let out = set_metadata(document, "GitHub", "#123").unwrap();

    assert_eq!(out, indoc! {"
            # T0042: Tool call header misaligned

            - **Status**: Todo
            - **Kind**: Bug
            - **Authors**: jean
            - **Date**: 2026-08-05
            - **GitHub**: #123

            Description.
        "});
}

#[test]
fn reports_a_document_with_no_metadata_block() {
    assert_eq!(
        set_metadata("# T0001: Bare\n\nProse.\n", "Status", "Done"),
        None
    );
}
