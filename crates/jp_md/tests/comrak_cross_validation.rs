//! Cross-validation of `Buffer`'s block segmentation against comrak.
//!
//! RFD 004 records the core risk of the streaming design: `Buffer` and comrak
//! must agree on block boundaries, or a block renders incorrectly.
//! The fuzz suite checks `Buffer`'s *self*-consistency (chunked vs. whole
//! input); this suite checks the property the streaming renderer actually
//! relies on: parsing each emitted segment as an independent document is
//! equivalent to parsing the whole document.
//!
//! Equivalence is checked by reassembling the emitted events into a document
//! (reapplying each event's visual indent) and canonicalizing both the original
//! and the reassembly through comrak's CommonMark formatter.

use std::fs;

use jp_md::buffer::{Buffer, Event};

/// Comrak options mirroring `Formatter::parse_options`, so the validation sees
/// the same extension set the production renderer uses.
fn comrak_options() -> comrak::Options<'static> {
    comrak::Options {
        extension: comrak::options::Extension {
            strikethrough: true,
            table: true,
            tasklist: true,
            superscript: true,
            underline: true,
            subscript: true,
            ..Default::default()
        },
        render: comrak::options::Render {
            width: 80,
            list_style: comrak::options::ListStyleType::Dash,
            prefer_fenced: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Canonicalize a markdown document through comrak's CommonMark formatter.
fn canonicalize(text: &str) -> String {
    let options = comrak_options();
    let arena = comrak::Arena::new();
    let ast = comrak::parse_document(&arena, text, &options);
    let mut out = String::new();
    comrak::format_commonmark(ast, &options, &mut out).expect("formatting a parsed AST succeeds");
    out
}

/// Collect all events for a document pushed as a single chunk.
///
/// Streaming is disabled: this suite asserts that segment-wise parsing equals
/// whole-document parsing, an equality `ParagraphChunk`s intentionally break.
fn whole_document_events(text: &str) -> Vec<Event> {
    let mut buffer = Buffer::new().with_streaming_paragraphs(false);
    buffer.push(text);
    let mut events: Vec<Event> = buffer.by_ref().collect();
    events.extend(buffer.flush_events());
    events
}

/// Collect all events for a document pushed one character at a time.
fn char_chunked_events(text: &str) -> Vec<Event> {
    let mut buffer = Buffer::new().with_streaming_paragraphs(false);
    let mut events = Vec::new();
    for (start, c) in text.char_indices() {
        buffer.push(&text[start..start + c.len_utf8()]);
        events.extend(buffer.by_ref());
    }
    events.extend(buffer.flush_events());
    events
}

/// Reassemble emitted events into a markdown document, reapplying each event's
/// visual indent.
fn reassemble(events: &[Event]) -> String {
    let mut out = String::new();
    for event in events {
        match event {
            Event::Block { content, indent }
            | Event::Flush { content, indent }
            | Event::FencedCodeLine { content, indent } => {
                out.push_str(&indent_lines(content, *indent));
            }
            Event::FencedCodeStart { indent, .. } => {
                out.push_str(&" ".repeat(*indent));
                out.push_str(&event.to_string());
                out.push('\n');
            }
            Event::FencedCodeEnd { fence, indent } => {
                out.push_str(&" ".repeat(*indent));
                out.push_str(fence);
                out.push('\n');
            }
            // Streaming is disabled in this suite, so a `ParagraphChunk` must
            // never reach here; fail loudly if one does (and satisfy the
            // `#[non_exhaustive]` enum).
            other => panic!("unexpected streaming event in non-streaming reassembly: {other:?}"),
        }
    }
    out
}

/// Prefix each non-blank line of `content` with `indent` spaces.
fn indent_lines(content: &str, indent: usize) -> String {
    if indent == 0 {
        return content.to_string();
    }
    let prefix = " ".repeat(indent);
    let mut out = String::new();
    for line in content.split_inclusive('\n') {
        if !line.trim_end_matches('\n').is_empty() {
            out.push_str(&prefix);
        }
        out.push_str(line);
    }
    out
}

/// Assert that segment-wise parsing of `document` is equivalent to
/// whole-document parsing, after comrak canonicalization of both sides.
#[track_caller]
fn assert_segmentation_equivalent(document: &str, name: &str) {
    let events = whole_document_events(document);
    let reassembled = reassemble(&events);

    let expected = canonicalize(document);
    let actual = canonicalize(&reassembled);

    assert_eq!(
        actual, expected,
        "comrak cross-validation failed for {name}\n--- events ---\n{events:#?}\n--- reassembled \
         ---\n{reassembled}"
    );
}

#[test]
fn fixtures_cross_validate_against_comrak() {
    let glob_pattern = format!("{}/tests/fixtures/*.md", env!("CARGO_MANIFEST_DIR"));
    let paths: Vec<_> = glob::glob(&glob_pattern)
        .expect("valid glob pattern")
        .filter_map(Result::ok)
        .collect();

    assert!(!paths.is_empty(), "No fixtures found in {glob_pattern}");

    for path in paths {
        let name = path.file_name().expect("file name").to_string_lossy();
        let content = fs::read_to_string(&path).expect("readable fixture");
        assert_segmentation_equivalent(&content, &name);
    }
}

/// Adversarial shapes absent from the fixtures: lines whose *prefix* parses as
/// a block starter while the complete line does not, and list tails with
/// mismatched markers.
/// Each document is validated against comrak and checked for chunked/whole
/// self-consistency at maximum fragmentation.
#[test]
fn adversarial_documents_cross_validate() {
    let documents = [
        ("atx_no_space_after_paragraph", "para\n#hello\n\nnext\n\n"),
        ("fence_info_with_backtick", "para\n```a`b\n\nnext\n\n"),
        (
            "unknown_html_tag_after_paragraph",
            "para\n<divx>\n\nnext\n\n",
        ),
        (
            "mixed_delimiter_list_tail",
            "1. one\n2. two\n3) three\n\npara\n",
        ),
        (
            "ordered_list_nonsequential_numbers",
            "5. five\n5. six\n5. seven\n\npara\n",
        ),
        ("bullet_list_with_ordered_tail", "- one\n- two\n1. three\n"),
        ("setext_after_paragraph", "Header\n===\n\nbody text\n\n"),
        (
            "thematic_break_lookalike",
            "para\n--- not a break\n\nnext\n\n",
        ),
        (
            "fenced_code_in_list_item",
            "- item\n\n  ```rust\n  fn main() {}\n  ```\n\n- next\n\npara\n",
        ),
    ];

    for (name, document) in documents {
        assert_segmentation_equivalent(document, name);

        let whole = whole_document_events(document);
        let chunked = char_chunked_events(document);
        assert_eq!(chunked, whole, "chunked/whole divergence for {name}");
    }
}
