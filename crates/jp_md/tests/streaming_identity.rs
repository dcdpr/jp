//! Buffer-half of RFD 089's byte-identity guarantee, over the fixture corpus.
//!
//! RFD 089 splits byte-identity into two halves: the buffer owns
//! *block-boundary* correctness (which bytes belong to a paragraph), and the
//! renderer owns *line* stability.
//! This suite covers the buffer's half: streaming a document must emit
//! `ParagraphChunk`s whose contents reconstruct exactly the same bytes as the
//! non-streaming `Block`s, with block boundaries unchanged.
//! The renderer's half — the accumulate/cut/emit-delta rule — is covered by
//! the `ChatRenderer` byte-identity tests in `jp_cli`, which need no markdown
//! corpus.

use std::fs;

use jp_md::buffer::{Buffer, Event};

/// Collect every event for `text` pushed one character at a time.
///
/// Per-character feeding is what makes the streaming path actually stream: were
/// the whole document pushed at once, every block's terminator would already be
/// present and the buffer would emit `Block`s and never a `ParagraphChunk`.
fn events(text: &str, streaming: bool) -> Vec<Event> {
    let mut buffer = Buffer::new().with_streaming_paragraphs(streaming);
    let mut out = Vec::new();
    for ch in text.chars() {
        buffer.push(&ch.to_string());
        out.extend(buffer.by_ref());
    }
    out.extend(buffer.flush_events());
    out
}

/// Reconstruct the document bytes from emitted events, reapplying each event's
/// visual indent.
///
/// `ParagraphChunk` content is appended verbatim (its indent is always 0), so a
/// paragraph's chunks reassemble into the same bytes a non-streaming `Block`
/// carries.
/// Indented events (list items, fenced code) are identical between the
/// streaming and non-streaming runs, so the comparison hinges only on paragraph
/// bytes.
fn reconstruct(events: &[Event]) -> String {
    let mut out = String::new();
    for event in events {
        match event {
            Event::Block { content, indent }
            | Event::Flush { content, indent }
            | Event::FencedCodeLine { content, indent } => {
                out.push_str(&indent_lines(content, *indent));
            }
            Event::ParagraphChunk { content, .. } => out.push_str(content),
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
            _ => {}
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

/// Streaming a fixture reconstructs the same bytes as non-streaming buffering,
/// over the shared corpus.
/// A realistic setext heading stays under the streaming threshold and renders
/// as today, so the documented over-threshold-setext exception does not arise
/// for these documents.
#[test]
fn fixtures_stream_identically_to_non_streaming() {
    let glob_pattern = format!("{}/tests/fixtures/*.md", env!("CARGO_MANIFEST_DIR"));
    let paths: Vec<_> = glob::glob(&glob_pattern)
        .expect("valid glob pattern")
        .filter_map(Result::ok)
        .collect();

    assert!(!paths.is_empty(), "No fixtures found in {glob_pattern}");

    for path in paths {
        let name = path.file_name().expect("file name").to_string_lossy();
        let content = fs::read_to_string(&path).expect("readable fixture");

        let streamed = reconstruct(&events(&content, true));
        let whole = reconstruct(&events(&content, false));

        assert_eq!(
            streamed, whole,
            "streaming buffer reconstruction differs from non-streaming for {name}"
        );
    }
}
