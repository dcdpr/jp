use indoc::indoc;

use super::*;
use crate::{Kind, Status};

/// A ticket with a description and no comments.
const PLAIN: &str = indoc! {"
    # Tool call header misaligned

    - **Status**: Todo
    - **Kind**: Bug
    - **Authors**: Jean Mertz
    - **Date**: 2026-08-05

    The header renders one column left of the body below 80 columns.
"};

/// The same ticket after two comments.
const WITH_COMMENTS: &str = indoc! {"
    # Tool call header misaligned

    - **Status**: In Progress
    - **Kind**: Bug
    - **Authors**: Jean Mertz
    - **Date**: 2026-08-05
    - **Implements**: 095

    The header renders one column left of the body below 80 columns.

    ## Comments

    -----

    - **From**: jean
    - **Date**: 2026-08-05T14:03:11Z

    Reproduced at 72 columns. Not at 80.

    -----

    - **From**: jp
    - **Date**: 2026-08-05T14:31:02Z
    - **Re**: #1

    The wrap calculation uses the pre-indent width.
"};

#[test]
fn reads_title_metadata_and_description() {
    let ticket = document(PLAIN).unwrap();

    assert_eq!(ticket.title, "Tool call header misaligned");
    assert_eq!(ticket.metadata.status, Status::Todo);
    assert_eq!(ticket.metadata.kind, Kind::Bug);
    assert_eq!(ticket.metadata.authors, "Jean Mertz");
    assert_eq!(ticket.metadata.date, "2026-08-05");
    assert_eq!(
        ticket.description,
        "The header renders one column left of the body below 80 columns."
    );
    assert!(ticket.comments.is_empty());
}

#[test]
fn reads_optional_metadata_fields() {
    let ticket = document(WITH_COMMENTS).unwrap();

    assert_eq!(ticket.metadata.status, Status::InProgress);
    assert_eq!(ticket.metadata.implements.as_deref(), Some("095"));
    assert_eq!(ticket.metadata.blocked_by, None);
    assert_eq!(ticket.metadata.github, None);
    assert!(ticket.metadata.labels.is_empty());
}

/// Labels are read as written, without checking them against the vocabulary: a
/// listing that hid a label the file carries would disagree with the file.
#[test]
fn reads_labels_as_written() {
    let source = indoc! {"
        # Labelled

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: john
        - **Date**: 2026-08-05
        - **Labels**: app/macos,  config , retired/label

        Description.
    "};

    let ticket = document(source).unwrap();

    assert_eq!(ticket.metadata.labels, [
        "app/macos",
        "config",
        "retired/label"
    ]);
}

/// An empty label line is no labels rather than one blank one.
#[test]
fn reads_an_empty_label_line_as_no_labels() {
    let source = indoc! {"
        # Labelled

        - **Status**: Todo
        - **Kind**: Bug
        - **Authors**: john
        - **Date**: 2026-08-05
        - **Labels**:

        Description.
    "};

    assert!(document(source).unwrap().metadata.labels.is_empty());
}

#[test]
fn reads_comments_in_order() {
    let ticket = document(WITH_COMMENTS).unwrap();

    assert_eq!(ticket.comments.len(), 2);
    assert_eq!(ticket.comments[0].from, "jean");
    assert_eq!(ticket.comments[0].date, "2026-08-05T14:03:11Z");
    assert_eq!(ticket.comments[0].re, None);
    assert_eq!(
        ticket.comments[0].body,
        "Reproduced at 72 columns. Not at 80."
    );
    assert_eq!(ticket.comments[1].from, "jp");
    assert_eq!(ticket.comments[1].re.as_deref(), Some("#1"));
    assert_eq!(
        ticket.comments[1].body,
        "The wrap calculation uses the pre-indent width."
    );
}

#[test]
fn description_stops_at_the_first_comment() {
    let ticket = document(WITH_COMMENTS).unwrap();

    assert!(!ticket.description.contains("## Comments"));
    assert!(ticket.description.ends_with("below 80 columns."));
}

/// A comment quoting a ticket file must not split the ticket it lives in.
#[test]
fn separators_inside_fenced_blocks_are_not_boundaries() {
    let source = indoc! {"
        # Quoting a ticket

        - **Status**: Todo
        - **Kind**: Chore
        - **Authors**: jean
        - **Date**: 2026-08-05

        Description.

        ## Comments

        -----

        - **From**: jp
        - **Date**: 2026-08-05T14:31:02Z

        The format looks like this:

        ```markdown
        -----

        - **From**: someone
        - **Date**: 2026-08-05T00:00:00Z

        Not a real comment.
        ```
    "};

    let ticket = document(source).unwrap();

    assert_eq!(ticket.comments.len(), 1);
    assert!(ticket.comments[0].body.contains("Not a real comment."));
}

/// A body that opens a four-backtick fence to quote three-backtick code stays
/// one comment.
#[test]
fn longer_fences_contain_shorter_ones() {
    let source = indoc! {"
        # Nested fences

        - **Status**: Todo
        - **Kind**: Chore
        - **Authors**: jean
        - **Date**: 2026-08-05

        Description.

        ## Comments

        -----

        - **From**: jp
        - **Date**: 2026-08-05T14:31:02Z

        ````markdown
        ```rust
        let x = 1;
        ```

        -----

        - **From**: nobody
        - **Date**: never

        Quoted.
        ````
    "};

    let ticket = document(source).unwrap();

    assert_eq!(ticket.comments.len(), 1);
    assert!(ticket.comments[0].body.contains("Quoted."));
}

#[test]
fn separator_without_a_metadata_block_is_not_a_boundary() {
    let source = indoc! {"
        # Horizontal rules

        - **Status**: Todo
        - **Kind**: Chore
        - **Authors**: jean
        - **Date**: 2026-08-05

        Above the rule.

        -----

        Below the rule.
    "};

    let ticket = document(source).unwrap();

    assert!(ticket.comments.is_empty());
    assert!(ticket.description.contains("Below the rule."));
}

/// The heading is decorative: the parser never consults it, so one written by
/// hand in a description is harmless.
#[test]
fn comments_heading_alone_creates_no_comments() {
    let source = indoc! {"
        # Decorative heading

        - **Status**: Todo
        - **Kind**: Chore
        - **Authors**: jean
        - **Date**: 2026-08-05

        ## Comments

        Still the description.
    "};

    let ticket = document(source).unwrap();

    assert!(ticket.comments.is_empty());
    assert!(ticket.description.contains("Still the description."));
}

#[test]
fn counts_comments_without_validating_the_header() {
    assert_eq!(comment_count(PLAIN), 0);
    assert_eq!(comment_count(WITH_COMMENTS), 2);
    assert_eq!(comment_count("no ticket here"), 0);
}

/// A ticket whose metadata block is missing a field still has a readable title,
/// and an append never needs the rest of the header.
#[test]
fn reads_the_title_without_validating_the_header() {
    let headerless = indoc! {"
        # Tool call header misaligned

        - **Status**: Todo
        - **Kind**: Bug
        - **Date**: 2026-08-05

        The header renders one column left of the body.
    "};

    assert_eq!(
        document(headerless),
        Err(ParseError::MissingField("Authors"))
    );
    assert_eq!(
        title(headerless).as_deref(),
        Some("Tool call header misaligned")
    );
    assert_eq!(title(PLAIN).as_deref(), Some("Tool call header misaligned"));
    assert_eq!(title("no ticket here"), None);
}

#[test]
fn rejects_a_document_without_a_title() {
    let source = "- **Status**: Todo\n";

    assert_eq!(document(source), Err(ParseError::MissingTitle));
}

#[test]
fn rejects_a_title_without_a_metadata_block() {
    let source = "# Bare\n\nJust prose.\n";

    assert_eq!(document(source), Err(ParseError::MissingMetadata));
}

#[test]
fn rejects_a_missing_required_field() {
    let source = indoc! {"
        # No kind

        - **Status**: Todo
        - **Authors**: jean
        - **Date**: 2026-08-05
    "};

    assert_eq!(document(source), Err(ParseError::MissingField("Kind")));
}

#[test]
fn rejects_an_unknown_status() {
    let source = indoc! {"
        # Bad status

        - **Status**: Blocked
        - **Kind**: Bug
        - **Authors**: jean
        - **Date**: 2026-08-05
    "};

    assert_eq!(
        document(source),
        Err(ParseError::InvalidValue {
            field: "Status",
            value: "Blocked".to_owned(),
        })
    );
}

#[test]
fn locates_the_metadata_block() {
    assert_eq!(metadata_range(PLAIN), Some(2..6));
    assert_eq!(metadata_range("# No metadata\n"), None);
}

#[test]
fn splits_metadata_lines() {
    assert_eq!(
        meta_line("- **Blocked by**: T-02wt0m3"),
        Some(("Blocked by", "T-02wt0m3"))
    );
    assert_eq!(meta_line("- **Re**:"), Some(("Re", "")));
    assert_eq!(meta_line("- not metadata"), None);
    assert_eq!(meta_line("  - **Indented**: value"), None);
}

/// A markdown formatter escapes a value that opens with `#`, since the bare
/// form would read as a heading.
#[test]
fn reads_a_value_through_its_markdown_escape() {
    assert_eq!(meta_line(r"- **Re**: \#1"), Some(("Re", "#1")));
    assert_eq!(meta_line("- **Re**: #1"), Some(("Re", "#1")));
}

#[test]
fn recognizes_separators() {
    assert!(is_separator("-----"));
    assert!(is_separator("----------"));
    assert!(is_separator("-----   "));
    assert!(!is_separator("----"));
    assert!(!is_separator(" -----"));
    assert!(!is_separator("--- x"));
}
