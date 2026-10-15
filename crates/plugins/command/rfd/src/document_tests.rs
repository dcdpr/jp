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

/// Older RFDs, written before `comfort` reflowed them, continue a wrapped value
/// without indenting it; markdown reads that as the same paragraph.
#[test]
fn an_unindented_continuation_belongs_to_the_field_above_it() {
    let text = "\
# RFD 014: Guide

- **Status**: Superseded
- **Extended by**: [RFD 015], [RFD 016],
[RFD 017]
- **Summary**: Guide for implementing attachment handlers: URL parsing, trait
methods, registration, and conventions.

## Summary
";
    let mut document = Document::parse(text);

    assert_eq!(
        document.header().wrapped("Summary").as_deref(),
        Some(
            "Guide for implementing attachment handlers: URL parsing, trait methods, \
             registration, and conventions."
        )
    );
    assert_eq!(document.header().citations("Extended by"), [
        "015", "016", "017"
    ]);

    assert_eq!(document.cite("Extended by", "018", "[RFD 018]"), Ok(true));
    assert!(document.uncite("Extended by", |id| id == "016"));
    document.set("Requires", "[RFD 001]").unwrap();

    assert_eq!(
        document.render(),
        "\
# RFD 014: Guide

- **Status**: Superseded
- **Extended by**: [RFD 015], [RFD 017], [RFD 018]
- **Requires**: [RFD 001]
- **Summary**: Guide for implementing attachment handlers: URL parsing, trait
methods, registration, and conventions.

## Summary
"
    );
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

/// Every edit below starts from this: a wrapped relationship, a field after it,
/// and the `Summary` RFD 001 keeps last.
const EDITED: &str = "\
<!-- A draft-time note. -->

# RFD 042: Ordered Things

- **Status**: Accepted
- **Category**: Design
- **Extended by**: [RFD 043](043-more-order.md), [RFD 044],
  [RFD 045]
- **Date**: 2026-03-07
- **Summary**: Keeps things in order by writing them down,
  one line at a time.

## Summary

Body.
";

fn edit(apply: impl FnOnce(&mut Document)) -> String {
    let mut document = Document::parse(EDITED);
    apply(&mut document);

    document.render()
}

#[test]
fn a_document_renders_back_unchanged() {
    assert_eq!(Document::parse(EDITED).render(), EDITED);
    assert_eq!(Document::parse("no newline").render(), "no newline");
}

#[test]
fn line_endings_are_kept() {
    let text = "# RFD 042: T\r\n\r\n- **Status**: Draft\r\n";
    let mut document = Document::parse(text);

    document.set("Status", "Discussion").unwrap();

    assert_eq!(
        document.render(),
        "# RFD 042: T\r\n\r\n- **Status**: Discussion\r\n"
    );
}

#[test]
fn set_replaces_a_field_wrapped_lines_included() {
    let out = edit(|document| {
        document.set("Extended by", "[RFD 046]").unwrap();
        document.set("Status", "Implemented").unwrap();
    });

    assert!(
        out.contains(
            "- **Status**: Implemented\n- **Category**: Design\n- **Extended by**: [RFD 046]\n- \
             **Date**: 2026-03-07\n"
        ),
        "{out}"
    );
}

/// RFD 001 keeps `Summary` last, so a field the block doesn't carry yet goes
/// above it.
#[test]
fn set_adds_a_missing_field_above_the_summary() {
    let out = edit(|document| document.set("Requires", "[RFD 072]").unwrap());

    assert!(
        out.contains("- **Date**: 2026-03-07\n- **Requires**: [RFD 072]\n- **Summary**: Keeps"),
        "{out}"
    );
}

#[test]
fn set_adds_a_missing_field_at_the_end_of_a_block_without_a_summary() {
    let mut document = Document::parse("# RFD D05: T\n\n- **Status**: Draft\n\n## Summary\n");

    document.set("Category", "Guide").unwrap();

    assert_eq!(
        document.render(),
        "# RFD D05: T\n\n- **Status**: Draft\n- **Category**: Guide\n\n## Summary\n"
    );
}

#[test]
fn a_document_without_metadata_refuses_a_new_field() {
    let mut document = Document::parse("# RFD 042: T\n\n## Summary\n");

    assert_eq!(document.set("Status", "Draft"), Err(NoMetadata));
    assert_eq!(
        document.cite("Requires", "001", "[RFD 001]"),
        Err(NoMetadata)
    );
}

#[test]
fn remove_takes_a_field_and_its_wrapped_lines() {
    let out = edit(|document| assert!(document.remove("Extended by")));

    assert!(
        out.contains("- **Category**: Design\n- **Date**: 2026-03-07\n"),
        "{out}"
    );
    assert!(!out.contains("RFD 045"), "{out}");
    assert!(!Document::parse(EDITED).remove("Requires"));
}

/// The new entry follows the last one, which a wrapped value keeps on its last
/// line, not its first.
#[test]
fn cite_appends_after_the_last_wrapped_line() {
    let out = edit(|document| {
        assert_eq!(
            document.cite("Extended by", "116", "[RFD 116](116-tooling.md)"),
            Ok(true)
        );
    });

    assert!(
        out.contains(
            "- **Extended by**: [RFD 043](043-more-order.md), [RFD 044],\n  [RFD 045], [RFD \
             116](116-tooling.md)\n- **Date**"
        ),
        "{out}"
    );
}

#[test]
fn citing_an_rfd_already_cited_changes_nothing() {
    let mut document = Document::parse(EDITED);

    assert_eq!(
        document.cite("Extended by", "045", "[RFD 045](045-x.md)"),
        Ok(false)
    );
    assert_eq!(document.render(), EDITED);
}

#[test]
fn cite_adds_a_missing_field_above_the_summary() {
    let out = edit(|document| {
        assert_eq!(
            document.cite("Supersedes", "033", "[RFD 033](033-x.md)"),
            Ok(true)
        );
    });

    assert!(
        out.contains(
            "- **Date**: 2026-03-07\n- **Supersedes**: [RFD 033](033-x.md)\n- **Summary**"
        ),
        "{out}"
    );
}

#[test]
fn uncite_drops_entries_wherever_they_wrap_to() {
    let out = edit(|document| assert!(document.uncite("Extended by", |id| id == "045")));

    assert!(
        out.contains(
            "- **Extended by**: [RFD 043](043-more-order.md), [RFD 044]\n- **Date**: 2026-03-07\n"
        ),
        "{out}"
    );
}

#[test]
fn uncite_removes_a_field_left_empty() {
    let out = edit(|document| assert!(document.uncite("Extended by", |_| true)));

    assert!(
        out.contains("- **Category**: Design\n- **Date**: 2026-03-07\n"),
        "{out}"
    );
    assert!(!out.contains("Extended by"), "{out}");
}

#[test]
fn uncite_without_a_match_changes_nothing() {
    let mut document = Document::parse(EDITED);

    assert!(!document.uncite("Extended by", |id| id == "999"));
    assert!(!document.uncite("Requires", |_| true));
    assert_eq!(document.render(), EDITED);
}

#[test]
fn the_heading_id_changes_and_the_title_stays() {
    let out = edit(|document| assert!(document.set_heading_id("097")));

    assert!(out.contains("\n# RFD 097: Ordered Things\n"), "{out}");

    let out = edit(|document| assert!(document.set_heading("042", "Kept in Order")));

    assert!(out.contains("\n# RFD 042: Kept in Order\n"), "{out}");
}

#[test]
fn a_document_without_a_heading_keeps_its_lines() {
    let mut document = Document::parse("- **Status**: Draft\n");

    assert!(!document.set_heading_id("097"));
    assert!(!document.set_heading("097", "Title"));
    assert_eq!(document.render(), "- **Status**: Draft\n");
}

#[test]
fn a_notice_goes_between_the_metadata_and_the_first_section() {
    let out = edit(|document| {
        document.insert_before_sections(&["> Noted.".to_owned()]);
    });

    assert!(
        out.ends_with("  one line at a time.\n\n> Noted.\n\n## Summary\n\nBody.\n"),
        "{out}"
    );
}

#[test]
fn a_notice_goes_at_the_end_of_a_document_without_sections() {
    let mut document = Document::parse("# RFD D05: T\n\n- **Status**: Draft");

    document.insert_before_sections(&["> Noted.".to_owned()]);

    assert_eq!(
        document.render(),
        "# RFD D05: T\n\n- **Status**: Draft\n\n> Noted.\n"
    );
}

#[test]
fn an_entry_cites_the_first_rfd_it_names() {
    assert_eq!(cited("[RFD 001][tickets]").as_deref(), Some("001"));
    assert_eq!(cited("[RFD D05](drafts/D05-x.md)").as_deref(), Some("D05"));
    assert_eq!(cited("the process"), None);
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
