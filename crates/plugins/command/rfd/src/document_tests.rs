use super::*;

/// The opening of a document, the way `comfort` lays it out: a long value wraps
/// onto lines indented past the list marker.
const DOCUMENT: &str = "\
# RFD 042: Ordered Things

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-07
- **Extends**: [RFD 001], [RFD 100]
- **Requires**: [RFD 072](072-command-plugin-system.md), [RFD D05],
  [RFD 077]
- **Summary**: Keeps things in order by writing them down,
  one line at a time.

## Summary

- **Status**: Draft
- **Requires**: RFD 999
";

fn status_of(content: &str) -> Option<String> {
    let lines: Vec<&str> = content.lines().collect();

    Header::of(&lines).field("Status")
}

#[test]
fn a_field_is_read_from_the_metadata_block() {
    let lines: Vec<&str> = DOCUMENT.lines().collect();
    let header = Header::of(&lines);

    assert_eq!(header.field("Status").as_deref(), Some("Accepted"));
    assert_eq!(header.field("Category").as_deref(), Some("Design"));
    assert_eq!(
        header.field("Authors").as_deref(),
        Some("Jean Mertz <git@jeanmertz.com>")
    );
    assert_eq!(header.field("Supersedes"), None);
}

/// RFD 001 quotes the metadata format further down; those lines are an example,
/// not the document's own metadata.
#[test]
fn lines_past_the_metadata_block_are_not_read() {
    let lines: Vec<&str> = DOCUMENT.lines().collect();
    let header = Header::of(&lines);

    assert_eq!(header.field("Status").as_deref(), Some("Accepted"));
    assert!(!header.citations("Requires").contains(&"999".to_owned()));
}

#[test]
fn a_wrapped_value_reads_as_one_line() {
    let lines: Vec<&str> = DOCUMENT.lines().collect();
    let header = Header::of(&lines);

    assert_eq!(
        header.wrapped("Summary").as_deref(),
        Some("Keeps things in order by writing them down, one line at a time.")
    );
    // `field` is the first line alone.
    assert_eq!(
        header.field("Summary").as_deref(),
        Some("Keeps things in order by writing them down,")
    );
}

/// A relationship that wraps keeps the ids on its continuation lines, whichever
/// way each one is linked.
#[test]
fn citations_are_read_across_wrapped_lines_in_order() {
    let lines: Vec<&str> = DOCUMENT.lines().collect();
    let header = Header::of(&lines);

    assert_eq!(header.citations("Extends"), ["001", "100"]);
    assert_eq!(header.citations("Requires"), ["072", "D05", "077"]);
    assert!(header.citations("Required by").is_empty());
}

#[test]
fn a_heading_before_any_metadata_means_there_is_none() {
    assert_eq!(
        status_of("# RFD 042: Title\n\n## Summary\n\n- **Status**: Draft\n"),
        None
    );
}

#[test]
fn an_empty_value_is_no_value() {
    assert_eq!(
        status_of("# RFD 042: Title\n\n- **Status**:\n- **Category**: Design\n"),
        None
    );
}

#[test]
fn the_title_comes_from_the_rfd_heading() {
    assert_eq!(title(DOCUMENT).as_deref(), Some("Ordered Things"));
    assert_eq!(
        title("<!-- note -->\n\n# RFD D05: A Draft\n").as_deref(),
        Some("A Draft")
    );
    // Templates and stray headings don't count.
    assert_eq!(title("# RFD NNN: TITLE\n"), None);
    assert_eq!(title("# Ordered Things\n"), None);
}

#[test]
fn referenced_labels_are_rfds_then_tickets_in_first_appearance_order() {
    let content = "\
See [RFD 072](072-command-plugin-system.md) and [the draft](drafts/D05-dev-plugin.md).
Again 072-command-plugin-system, and T-02wt0kx, then 001-jp-rfd-process.
Dated 2026-08-05, which is no link. T-02wt0kx twice.
";

    assert_eq!(referenced_labels(content), [
        "072",
        "D05",
        "001",
        "T-02wt0kx"
    ]);
}
