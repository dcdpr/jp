use super::*;

/// The holdback invariant the llamacpp provider relies on: with no tag
/// present, `handle` releases everything except a tail of `tag_len - 1`
/// bytes (in case a tag opener straddles the next chunk), and `finalize`
/// releases the remainder. Consumers must treat the unreleased tail as
/// still-pending: releasing it only after a tool-call boundary would split
/// the final word across paragraphs.
#[test]
fn idle_holds_back_seven_byte_tail() {
    let mut x = ReasoningExtractor::default();
    x.handle("hello world"); // 11 bytes, no tag
    // 11 - 7 = 4 bytes released; the last 7 are held back.
    assert_eq!(x.other, "hell");
    assert_eq!(x.reasoning, "");
    x.finalize();
    assert_eq!(x.other, "hello world");
    assert_eq!(x.reasoning, "");
}

/// A complete open/close tag pair segments the stream into the
/// `other` / `reasoning` / `other` buckets.
#[test]
fn complete_tags_split_buckets() {
    let mut x = ReasoningExtractor::default();
    x.handle("pre");
    x.handle("<think>\nx");
    x.handle("</think>\ny");
    assert_eq!(x.other, "prey");
    assert_eq!(x.reasoning, "x");
}

/// The case the holdback exists for: an opener tag straddling a chunk
/// boundary. Nothing may be emitted until the tag is resolved.
#[test]
fn opener_split_across_chunks() {
    let mut x = ReasoningExtractor::default();
    x.handle("ab<thi"); // 6 bytes: all held (buf < holdback)
    assert_eq!(x.other, "");
    x.handle("nk>\nx"); // completes the opener
    assert_eq!(x.other, "ab");
    assert_eq!(x.reasoning, "");
    x.handle("</think>\ny"); // closes the block
    assert_eq!(x.other, "aby");
    assert_eq!(x.reasoning, "x");
}

/// A stream that ends inside an open block: `finalize` treats the
/// remainder as reasoning (no closer present).
#[test]
fn finalize_with_unclosed_block() {
    let mut x = ReasoningExtractor::default();
    x.handle("ab <think>\nreason");
    assert_eq!(x.other, "ab ");
    x.finalize();
    assert_eq!(x.other, "ab ");
    assert_eq!(x.reasoning, "reason");
}
