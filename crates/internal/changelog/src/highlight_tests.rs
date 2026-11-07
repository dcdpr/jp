use std::num::NonZeroU64;

use indoc::indoc;

use super::*;

#[test]
fn a_highlight_holds_a_title_the_authors_to_credit_and_its_prose() {
    let highlight = parse(indoc! {"
        # Unified editor and inline reply widget

        - **Authors**: @JeanMertz

        Every prompt in JP now runs through a single editor service, and short
        replies can be written inline without leaving the conversation view.
    "})
    .unwrap();

    assert_eq!(highlight, Highlight {
        title: "Unified editor and inline reply widget".to_owned(),
        authors: vec!["@JeanMertz".to_owned()],
        credits: None,
        body: indoc! {"
            Every prompt in JP now runs through a single editor service, and short
            replies can be written inline without leaving the conversation view."}
        .to_owned(),
    });
}

#[test]
fn each_author_is_a_field_of_its_own() {
    let highlight = parse(indoc! {"
        # Tool calls fan out

        - **Authors**: @JeanMertz
        - **Authors**: @jdoe

        The assistant calls one tool on many inputs at once.
    "})
    .unwrap();

    assert_eq!(highlight.authors, vec![
        "@JeanMertz".to_owned(),
        "@jdoe".to_owned()
    ]);
}

#[test]
fn credits_replace_the_credits_history_names() {
    let highlight = parse(indoc! {"
        # Tool calls fan out

        - **Authors**: @JeanMertz
        - **Credits**: #1182
        - **Credits**: #1183

        The assistant calls one tool on many inputs at once.
    "})
    .unwrap();

    assert_eq!(
        highlight.credits,
        Some(vec![
            ListedCredit::ChangeRequest(NonZeroU64::new(1182).unwrap()),
            ListedCredit::ChangeRequest(NonZeroU64::new(1183).unwrap())
        ])
    );
}

#[test]
fn the_title_authors_and_prose_are_read_without_the_space_around_them() {
    let highlight = parse(
        "\n#   Tool calls fan out  \n- **Authors**:   @JeanMertz  \n\n\nThe assistant calls one \
         tool on many inputs at once.\n\n",
    )
    .unwrap();

    assert_eq!(highlight.title, "Tool calls fan out");
    assert_eq!(highlight.authors, vec!["@JeanMertz".to_owned()]);
    assert_eq!(
        highlight.body,
        "The assistant calls one tool on many inputs at once."
    );
}

/// A highlight opens with a level-one heading, written as Markdown writes one:
/// `#`, a space, and the title.
#[test]
fn a_highlight_opens_with_its_title() {
    for untitled in [
        "- **Authors**: @JeanMertz\n\nProse.\n",
        "#\n\n- **Authors**: @JeanMertz\n\nProse.\n",
        "## Tool calls\n\n- **Authors**: @JeanMertz\n\nProse.\n",
        "#Tool calls\n\n- **Authors**: @JeanMertz\n\nProse.\n",
        "Prose first.\n\n# Tool calls\n\n- **Authors**: @JeanMertz\n",
        "",
    ] {
        assert_eq!(
            parse(untitled),
            Err(HighlightError::NoTitle),
            "{untitled:?}"
        );
    }
    assert_eq!(
        HighlightError::NoTitle.to_string(),
        "no title: a highlight opens with `# Title`, which heads its section of the release notes"
    );
}

#[test]
fn a_highlight_credits_at_least_one_author() {
    let error = parse("# Tool calls fan out\n\nProse.\n").unwrap_err();

    assert_eq!(error, HighlightError::NoAuthors);
    assert_eq!(
        error.to_string(),
        "no authors: list each person to credit for the feature below the title, as `- \
         **Authors**: @JeanMertz`"
    );
}

#[test]
fn every_author_names_someone() {
    let error = parse(indoc! {"
        # Tool calls fan out

        - **Authors**: @JeanMertz
        - **Authors**:

        Prose.
    "})
    .unwrap_err();

    assert_eq!(error, HighlightError::EmptyAuthor { line: 4 });
}

#[test]
fn a_highlight_has_no_type() {
    let error = parse(indoc! {"
        # Tool calls fan out

        - **Type**: added
        - **Authors**: @JeanMertz

        Prose.
    "})
    .unwrap_err();

    assert_eq!(error, HighlightError::UnknownField {
        line: 3,
        key: "Type".to_owned()
    });
    assert_eq!(
        error.to_string(),
        "line 3 lists `Type`, and the fields a highlight takes are `Authors` and `Credits`"
    );
}

#[test]
fn a_misspelled_field_is_an_error_rather_than_ignored() {
    let error = parse("# Tool calls fan out\n\n- **Author**: @JeanMertz\n\nProse.\n").unwrap_err();

    assert_eq!(error, HighlightError::UnknownField {
        line: 3,
        key: "Author".to_owned()
    });
}

#[test]
fn a_credit_names_a_change_request_or_a_commit() {
    let error = parse(indoc! {"
        # Tool calls fan out

        - **Authors**: @JeanMertz
        - **Credits**: @jdoe

        Prose.
    "})
    .unwrap_err();

    assert_eq!(error, HighlightError::InvalidCredit {
        line: 4,
        value: "@jdoe".to_owned()
    });
}

#[test]
fn a_highlight_needs_prose() {
    assert_eq!(
        parse("# Tool calls fan out\n\n- **Authors**: @JeanMertz\n\n  \n"),
        Err(HighlightError::EmptyBody)
    );
}

#[test]
fn windows_line_endings_and_a_byte_order_mark_are_read_as_usual() {
    let highlight =
        parse("\u{feff}# Fan out\r\n\r\n- **Authors**: @JeanMertz\r\n\r\nProse.\r\n").unwrap();

    assert_eq!(highlight.title, "Fan out");
    assert_eq!(highlight.authors, vec!["@JeanMertz".to_owned()]);
    assert_eq!(highlight.body, "Prose.");
}

#[test]
fn a_highlight_carries_no_upgrade_section() {
    let error = parse(indoc! {"
        # Plugins are found by their manifest

        - **Authors**: @JeanMertz

        JP reads the manifest each plugin binary embeds.

        BREAKING CHANGE: Command plugins are found by their manifest

        Rebuild your plugins against this release.
    "})
    .unwrap_err();

    assert_eq!(error, HighlightError::Marker {
        line: 7,
        marker: "BREAKING CHANGE:",
    });
}

#[test]
fn a_token_starting_any_line_of_a_highlight_is_an_error() {
    // The rule fragments follow, where a line starting with the token is never
    // text, with no line left where a highlight could open an upgrade section.
    let error = parse(indoc! {"
        # Breaking changes in commit messages

        - **Authors**: @JeanMertz

        A commit can mark a breaking change:

        ```text
        BREAKING-CHANGE: the flag is gone
        ```
    "})
    .unwrap_err();

    assert_eq!(error, HighlightError::Marker {
        line: 8,
        marker: "BREAKING-CHANGE:",
    });
}

#[test]
fn a_token_that_does_not_start_its_line_is_text() {
    let highlight = parse(indoc! {"
        # Upgrade sections

        - **Authors**: @JeanMertz

        `BREAKING CHANGE:` in inline code, and
         BREAKING CHANGE: indented, are both text.
    "})
    .unwrap();

    assert_eq!(highlight.body, indoc! {"
        `BREAKING CHANGE:` in inline code, and
         BREAKING CHANGE: indented, are both text."});
}

/// Release notes place many highlights in one document, where two defining one
/// label differently would collide, so the prose comes with its links inline.
#[test]
fn a_highlights_links_are_inline() {
    let highlight = parse(indoc! {"
        # Tool calls fan out

        - **Authors**: @JeanMertz

        The assistant calls [one tool] on many inputs at once.

        [one tool]: https://example.com/tools
    "})
    .unwrap();

    assert_eq!(
        highlight.body,
        "The assistant calls [one tool](https://example.com/tools) on many inputs at once."
    );
}

#[test]
fn a_highlight_of_definitions_alone_has_no_prose() {
    assert_eq!(
        parse("# Fan out\n\n- **Authors**: @JeanMertz\n\n[docs]: https://example.com\n"),
        Err(HighlightError::EmptyBody)
    );
}

#[test]
fn a_misplaced_token_says_where_the_section_goes_instead() {
    assert_eq!(
        HighlightError::Marker {
            line: 7,
            marker: "BREAKING CHANGE:",
        }
        .to_string(),
        "line 7 starts with `BREAKING CHANGE:`, and a highlight has no upgrade section; write the \
         section in the fragment for the change, or, to show the token, indent the line or put \
         the token in inline code"
    );
}
