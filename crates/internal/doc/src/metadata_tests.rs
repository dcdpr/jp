use indoc::indoc;

use super::*;

/// The opening of a document, the way `comfort` lays it out: a long value wraps
/// onto a line indented past the list marker.
const DOCUMENT: &str = indoc! {"
    # Ubiquitous Language: Conversation

    - **Status**: Reference
    - **Category**: glossary
    - **Label**: domain=conversation
    - **Label**: relevant-rfd=064
    - **Summary**: Canonical definitions for Conversation, Turn, Thread, and Event
      Stream.

    ## Turn

    - **Status**: Snapshot
"};

fn header_of(text: &str) -> (Vec<&str>, Range<usize>) {
    let lines: Vec<&str> = text.lines().collect();
    let block = block(&lines);

    (lines, block)
}

#[test]
fn splits_a_field_line() {
    assert_eq!(
        split("- **Promoted to**: RFD 042"),
        Some(("Promoted to", "RFD 042"))
    );
    assert_eq!(split("- **Re**:"), Some(("Re", "")));
    assert_eq!(
        split("- **Label**:  client = cli "),
        Some(("Label", "client = cli"))
    );
    assert_eq!(split("- not metadata"), None);
    assert_eq!(split("  - **Indented**: value"), None);
}

/// A value opening with `#` reaches the parser bare: it sits mid-line, where no
/// markdown formatter has reason to escape it.
#[test]
fn splits_a_value_that_opens_with_a_hash() {
    assert_eq!(split("- **Re**: #1"), Some(("Re", "#1")));
}

#[test]
fn writes_a_field_line() {
    assert_eq!(line("Status", "Reference"), "- **Status**: Reference");
}

#[test]
fn unescapes_what_a_formatter_escaped() {
    assert_eq!(unescape(r"package=jp\_config"), "package=jp_config");
    assert_eq!(unescape(r"\*bold\*"), "*bold*");
    assert_eq!(unescape(r"C:\path"), r"C:\path");
    assert_eq!(unescape(r"trailing\"), r"trailing\");
    assert_eq!(unescape("plain"), "plain");
}

#[test]
fn the_block_runs_from_the_first_field_to_the_blank_line() {
    let (_, block) = header_of(DOCUMENT);

    assert_eq!(block, 2..8);
}

#[test]
fn a_heading_before_any_field_means_there_is_no_block() {
    let (_, block) = header_of("# Title\n\n## Turn\n\n- **Status**: Snapshot\n");

    assert!(block.is_empty());
}

#[test]
fn a_field_is_read_from_the_block() {
    let (lines, _) = header_of(DOCUMENT);
    let header = Header::of(&lines);

    assert_eq!(header.field("Status").as_deref(), Some("Reference"));
    assert_eq!(header.field("Category").as_deref(), Some("glossary"));
    assert_eq!(header.field("Parent"), None);
}

/// A field line quoted further down is an example, not the document's own
/// metadata.
#[test]
fn lines_past_the_block_are_not_read() {
    let (lines, _) = header_of(DOCUMENT);

    assert_eq!(
        Header::of(&lines).field("Status").as_deref(),
        Some("Reference")
    );
}

#[test]
fn a_repeated_field_reads_its_first_line() {
    let (lines, _) = header_of(DOCUMENT);

    assert_eq!(
        Header::of(&lines).field("Label").as_deref(),
        Some("domain=conversation")
    );
}

#[test]
fn a_wrapped_value_reads_as_one_line() {
    let (lines, _) = header_of(DOCUMENT);
    let header = Header::of(&lines);

    assert_eq!(
        header.wrapped("Summary").as_deref(),
        Some("Canonical definitions for Conversation, Turn, Thread, and Event Stream.")
    );
    assert_eq!(
        header.field("Summary").as_deref(),
        Some("Canonical definitions for Conversation, Turn, Thread, and Event")
    );
}

#[test]
fn an_empty_value_is_no_value() {
    let lines = ["# Title", "", "- **Status**:", "- **Category**: guide"];
    let header = Header::of(&lines);

    assert_eq!(header.field("Status"), None);
    assert_eq!(header.wrapped("Status"), None);
}

/// A hand-edited file can spell a key in any case, and still reads.
#[test]
fn keys_match_without_case() {
    let lines = [
        "# Title",
        "",
        "- **status**: Snapshot",
        "- **CATEGORY**: guide",
    ];
    let header = Header::of(&lines);

    assert_eq!(header.field("Status").as_deref(), Some("Snapshot"));
    assert_eq!(header.wrapped("category").as_deref(), Some("guide"));
}

#[test]
fn fields_lists_every_line_that_opens_one() {
    let (lines, _) = header_of(DOCUMENT);

    let fields: Vec<(String, String)> = Header::of(&lines)
        .fields()
        .into_iter()
        .map(|field| (field.key, field.value))
        .collect();

    assert_eq!(fields, [
        ("Status".to_owned(), "Reference".to_owned()),
        ("Category".to_owned(), "glossary".to_owned()),
        ("Label".to_owned(), "domain=conversation".to_owned()),
        ("Label".to_owned(), "relevant-rfd=064".to_owned()),
        (
            "Summary".to_owned(),
            "Canonical definitions for Conversation, Turn, Thread, and Event Stream.".to_owned()
        ),
    ]);
}

/// A value `comfort` wrapped onto a second line keeps every field after it.
#[test]
fn a_wrapped_value_does_not_end_the_block() {
    let lines = [
        "# Title",
        "",
        "- **Authors**: Jane Doe <jane@example.com>, John Doe",
        "  <john@example.com>",
        "- **Label**: type=bug",
    ];
    let header = Header::of(&lines);

    assert_eq!(
        header.wrapped("Authors").as_deref(),
        Some("Jane Doe <jane@example.com>, John Doe <john@example.com>")
    );
    assert_eq!(header.values("Label"), ["type=bug"]);
}

#[test]
fn values_keeps_an_empty_one() {
    let lines = ["# Title", "", "- **Label**:", "- **label**: type=bug"];

    assert_eq!(Header::of(&lines).values("Label"), ["", "type=bug"]);
}

/// A `- **` line without its `**:` is no field, and neither is what continues
/// it.
#[test]
fn a_line_that_is_not_a_field_opens_nothing() {
    let lines = ["- **Status**: Snapshot", "- **Broken", "  still broken"];

    let fields = Header::of(&lines).fields();

    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].value, "Snapshot");
    assert_eq!(fields[0].lines, 0..1);
}

#[test]
fn a_field_owns_its_continuation_lines() {
    let (lines, _) = header_of(DOCUMENT);
    let header = Header::of(&lines);

    assert!(header.is_field_line("Summary", 6));
    assert!(header.is_field_line("Summary", 7));
    assert!(!header.is_field_line("Summary", 5));
    assert!(!header.is_field_line("Summary", 9));
}

#[test]
fn a_document_renders_back_unchanged() {
    assert_eq!(Document::parse(DOCUMENT).render(), DOCUMENT);
    assert_eq!(Document::parse("no newline").render(), "no newline");
}

#[test]
fn line_endings_are_kept() {
    let mut document = Document::parse("# Title\r\n\r\n- **Status**: Snapshot\r\n");

    document.set("Status", "Reference").unwrap();

    assert_eq!(
        document.render(),
        "# Title\r\n\r\n- **Status**: Reference\r\n"
    );
}

#[test]
fn set_replaces_a_field_wrapped_lines_included() {
    let mut document = Document::parse(DOCUMENT);

    document.set("Summary", "Shorter.").unwrap();

    assert!(
        document
            .render()
            .contains("- **Label**: relevant-rfd=064\n- **Summary**: Shorter.\n\n## Turn"),
        "{}",
        document.render()
    );
}

/// The block keeps `Summary` last, so a field it doesn't carry yet goes above
/// it.
#[test]
fn set_adds_a_missing_field_above_the_summary() {
    let mut document = Document::parse(DOCUMENT);

    document.set("Parent", "W-0a1b2c3").unwrap();

    assert!(
        document
            .render()
            .contains("- **Label**: relevant-rfd=064\n- **Parent**: W-0a1b2c3\n- **Summary**"),
        "{}",
        document.render()
    );
}

#[test]
fn set_adds_a_missing_field_at_the_end_of_a_block_without_a_summary() {
    let mut document = Document::parse("# Title\n\n- **Status**: Snapshot\n\n## Turn\n");

    document.set("Category", "research").unwrap();

    assert_eq!(
        document.render(),
        "# Title\n\n- **Status**: Snapshot\n- **Category**: research\n\n## Turn\n"
    );
}

#[test]
fn a_document_without_a_block_refuses_a_new_field() {
    let mut document = Document::parse("# Title\n\n## Turn\n");

    assert_eq!(document.set("Status", "Snapshot"), Err(NoMetadata));
    assert_eq!(document.render(), "# Title\n\n## Turn\n");
}

#[test]
fn set_rewrites_a_key_spelled_in_another_case() {
    let mut document = Document::parse("# Title\n\n- **status**: Snapshot\n");

    document.set("Status", "Reference").unwrap();

    assert_eq!(document.render(), "# Title\n\n- **Status**: Reference\n");
}

/// The values land where the first old line was, so relabelling doesn't shuffle
/// the block.
#[test]
fn set_repeated_replaces_every_line_in_place() {
    let mut document = Document::parse(DOCUMENT);

    document
        .set_repeated("Label", &["domain=llm".to_owned()])
        .unwrap();

    assert!(
        document.render().contains(
            "- **Category**: glossary\n- **Label**: domain=llm\n- **Summary**: Canonical"
        ),
        "{}",
        document.render()
    );
}

#[test]
fn set_repeated_adds_a_missing_field_above_the_summary() {
    let mut document = Document::parse(DOCUMENT);
    document.set_repeated("Label", &[]).unwrap();

    document
        .set_repeated("Label", &["a=1".to_owned(), "b=2".to_owned()])
        .unwrap();

    assert!(
        document.render().contains(
            "- **Category**: glossary\n- **Label**: a=1\n- **Label**: b=2\n- **Summary**"
        ),
        "{}",
        document.render()
    );
}

#[test]
fn set_repeated_with_no_values_on_a_missing_field_changes_nothing() {
    let mut document = Document::parse("# Title\n\n- **Status**: Snapshot\n");

    document.set_repeated("Label", &[]).unwrap();

    assert_eq!(document.render(), "# Title\n\n- **Status**: Snapshot\n");
    assert_eq!(
        Document::parse("# Title\n\nProse.\n").set_repeated("Label", &[]),
        Err(NoMetadata)
    );
}

#[test]
fn remove_takes_a_field_and_its_wrapped_lines() {
    let mut document = Document::parse(DOCUMENT);

    assert!(document.remove("Summary"));
    assert!(!document.remove("Parent"));
    assert!(
        document
            .render()
            .contains("- **Label**: relevant-rfd=064\n\n## Turn"),
        "{}",
        document.render()
    );
}

/// The text follows the value's end, which a wrapped value keeps on its last
/// line, not its first.
#[test]
fn append_extends_the_last_wrapped_line() {
    let mut document = Document::parse(DOCUMENT);

    assert!(document.append("Summary", " See also W-0a1b2c3."));
    assert!(!document.append("Parent", "W-0a1b2c3"));
    assert!(
        document
            .render()
            .contains("  Stream. See also W-0a1b2c3.\n\n## Turn"),
        "{}",
        document.render()
    );
}

#[test]
fn a_notice_goes_between_the_block_and_the_first_section() {
    let mut document = Document::parse(DOCUMENT);

    document.insert_before_sections(&["> Archived.".to_owned()]);

    assert!(
        document
            .render()
            .contains("  Stream.\n\n> Archived.\n\n## Turn\n"),
        "{}",
        document.render()
    );
}

#[test]
fn a_notice_goes_at_the_end_of_a_document_without_sections() {
    let mut document = Document::parse("# Title\n\n- **Status**: Snapshot");

    document.insert_before_sections(&["> Archived.".to_owned()]);

    assert_eq!(
        document.render(),
        "# Title\n\n- **Status**: Snapshot\n\n> Archived.\n"
    );
}

#[test]
fn a_line_edited_in_place_keeps_the_line_endings() {
    let mut document = Document::parse("# Title\r\n\r\n- **Status**: Snapshot\r\n");

    document.lines_mut()[0] = "# Renamed".to_owned();

    assert_eq!(
        document.render(),
        "# Renamed\r\n\r\n- **Status**: Snapshot\r\n"
    );
}
