use super::*;
use crate::buffer::{Buffer, Event, FenceType};

#[test]
fn orphaned_fence_converts_to_block() {
    // When a paragraph has an embedded fence and the next event is a
    // bare FencedCodeStart, the fixup converts it to a Block.
    let input = concat!(
        "1. First item\n",
        "2. Second item\n",
        "3. Some text, let me re-read:```rust\n",
        "\n",
        "```\n",
        "\n",
        "This is a regular paragraph after the list.\n",
        "\n",
    );

    let mut buf = Buffer::new();
    buf.push(input);

    let mut fixups = Fixups::new(vec![Box::new(OrphanedFenceFixup::new())]);
    let events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();

    // The paragraph after the list should be present, not swallowed as code.
    let has_paragraph = events.iter().any(
        |e| matches!(e, Event::Block { content, .. } if content.contains("regular paragraph")),
    );
    assert!(
        has_paragraph,
        "Paragraph after the list should not be swallowed.\nEvents: {events:#?}"
    );

    // There should be NO FencedCodeStart — the orphaned fence was converted.
    let has_fence_start = events
        .iter()
        .any(|e| matches!(e, Event::FencedCodeStart { .. }));
    assert!(
        !has_fence_start,
        "Orphaned fence should not produce FencedCodeStart.\nEvents: {events:#?}"
    );
}

#[test]
fn real_code_block_not_suppressed() {
    // A real code block (with language tag) after a normal paragraph
    // should NOT be affected by the fixup.
    let input = "Some text.\n\n```rust\nfn main() {}\n```\n\n";

    let mut buf = Buffer::new();
    buf.push(input);

    let mut fixups = Fixups::new(vec![Box::new(OrphanedFenceFixup::new())]);
    let events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();

    let has_fence_start = events
        .iter()
        .any(|e| matches!(e, Event::FencedCodeStart { .. }));
    assert!(
        has_fence_start,
        "Real code block should produce FencedCodeStart.\nEvents: {events:#?}"
    );
}

#[test]
fn fence_escalation_rewrites_lengths() {
    let input = "```rust\nfn main() {}\n```\n";

    let mut buf = Buffer::new();
    buf.push(input);

    let mut fixups = Fixups::new(vec![Box::new(FenceEscalationFixup)]);
    let events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();

    // Opening fence should be escalated to 5.
    assert!(
        matches!(&events[0], Event::FencedCodeStart {
            fence_length: 5,
            ..
        }),
        "Opening fence should be escalated to 5.\nEvents: {events:#?}"
    );

    // Closing fence should also be 5 backticks.
    let closing = events
        .iter()
        .find(|e| matches!(e, Event::FencedCodeEnd { .. }));
    assert_eq!(
        closing,
        Some(&Event::fenced_code_end("`````")),
        "Closing fence should be escalated to 5.\nEvents: {events:#?}"
    );
}

#[test]
fn fence_escalation_preserves_longer_fences() {
    // A 6-backtick fence should stay at 6 (already > 5).
    let input = "``````rust\ncode\n``````\n";

    let mut buf = Buffer::new();
    buf.push(input);

    let mut fixups = Fixups::new(vec![Box::new(FenceEscalationFixup)]);
    let events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();

    assert!(
        matches!(&events[0], Event::FencedCodeStart {
            fence_length: 6,
            ..
        }),
        "6-backtick fence should stay at 6.\nEvents: {events:#?}"
    );
}

#[test]
fn fence_escalation_handles_tildes() {
    let input = "~~~python\nprint()\n~~~\n";

    let mut buf = Buffer::new();
    buf.push(input);

    let mut fixups = Fixups::new(vec![Box::new(FenceEscalationFixup)]);
    let events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();

    assert!(
        matches!(&events[0], Event::FencedCodeStart {
            fence_type: FenceType::Tilde,
            fence_length: 5,
            ..
        }),
        "Tilde fence should be escalated to 5.\nEvents: {events:#?}"
    );

    let closing = events
        .iter()
        .find(|e| matches!(e, Event::FencedCodeEnd { .. }));
    assert_eq!(
        closing,
        Some(&Event::fenced_code_end("~~~~~")),
        "Tilde closing should be escalated.\nEvents: {events:#?}"
    );
}

#[test]
fn bare_fence_after_normal_paragraph_not_suppressed() {
    // A bare fence (no language) after a normal paragraph (no embedded
    // fences) should still open a code block.
    let input = "Normal paragraph.\n\n```\nsome code\n```\n\n";

    let mut buf = Buffer::new();
    buf.push(input);

    let mut fixups = Fixups::new(vec![Box::new(OrphanedFenceFixup::new())]);
    let events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();

    let has_fence_start = events
        .iter()
        .any(|e| matches!(e, Event::FencedCodeStart { .. }));
    assert!(
        has_fence_start,
        "Bare fence after normal paragraph should open a code block.\nEvents: {events:#?}"
    );
}

#[test]
fn paragraph_chunk_embedded_fence_flag_accumulates_then_suppresses() {
    // A streamed top-level paragraph ending in an embedded fence sets the
    // embedded-fence flag from its chunks, so the following bare fence is
    // treated as an orphaned close (converted to a Block, not a code block).
    let mut fixup = OrphanedFenceFixup::new();

    for chunk in [
        Event::paragraph_chunk("Here is some code: ", false),
        Event::paragraph_chunk("look:```rust", false),
        Event::paragraph_chunk("\n", true),
    ] {
        assert!(matches!(
            fixup.process(chunk),
            Some(Event::ParagraphChunk { .. })
        ));
    }

    let converted = fixup.process(Event::FencedCodeStart {
        language: String::new(),
        fence_type: FenceType::Backtick,
        fence_length: 3,
        indent: 0,
    });
    assert!(
        matches!(converted, Some(Event::Block { .. })),
        "orphaned fence after a streamed paragraph should convert to a Block, got: {converted:?}"
    );
}

#[test]
fn paragraph_chunk_without_embedded_fence_leaves_following_fence() {
    // A streamed paragraph with no embedded fence must NOT suppress a real
    // following code block.
    let mut fixup = OrphanedFenceFixup::new();
    for chunk in [
        Event::paragraph_chunk("Just some prose with no fence ", false),
        Event::paragraph_chunk("at all here.\n", true),
    ] {
        assert!(matches!(
            fixup.process(chunk),
            Some(Event::ParagraphChunk { .. })
        ));
    }

    let next = fixup.process(Event::FencedCodeStart {
        language: String::new(),
        fence_type: FenceType::Backtick,
        fence_length: 3,
        indent: 0,
    });
    assert!(
        matches!(next, Some(Event::FencedCodeStart { .. })),
        "a real code block after a fence-free paragraph must be preserved, got: {next:?}"
    );
}

/// Table-driven cases for [`SplitCodeSpanFixup`].
///
/// Each case feeds `input` events through a fresh fixup and asserts the exact
/// output events, so every guard and repair is pinned byte-for-byte.
/// Add new edge cases by appending to the table.
///
/// Chunk boundaries in the cases respect the buffer's guarantee that a backtick
/// run is never split across chunks (chunks end only at inline ground state or
/// hold the open construct to the terminal chunk).
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a flat case table; length is the point"
)]
fn split_code_span_cases() {
    struct Case {
        name: &'static str,
        input: Vec<Event>,
        want: Vec<Event>,
    }

    fn pc(content: impl Into<String>, last: bool) -> Event {
        Event::paragraph_chunk(content, last)
    }

    let fence_start = || Event::FencedCodeStart {
        language: "rust".into(),
        fence_type: FenceType::Backtick,
        fence_length: 3,
        indent: 0,
    };

    let cases = vec![
        Case {
            name: "real-world split: orphaned closer escaped, later spans restored",
            input: vec![
                pc("helpers that delegate to ", false),
                pc("`_rfd-next-draft-slot` for drafts and `_rfd-next-", true),
                pc(
                    "number` for permanent RFDs. I'm extracting `_rfd-priority-rewrite` too.",
                    true,
                ),
            ],
            want: vec![
                pc("helpers that delegate to ", false),
                pc("`_rfd-next-draft-slot` for drafts and `_rfd-next-", true),
                pc(
                    "number\\` for permanent RFDs. I'm extracting `_rfd-priority-rewrite` too.",
                    true,
                ),
            ],
        },
        Case {
            name: "plain paragraphs untouched",
            input: vec![
                pc("just some prose ", false),
                pc("across two chunks.", true),
                pc("and another paragraph.", true),
            ],
            want: vec![
                pc("just some prose ", false),
                pc("across two chunks.", true),
                pc("and another paragraph.", true),
            ],
        },
        Case {
            name: "balanced spans do not arm; leading span in next paragraph kept",
            input: vec![pc("uses `foo` properly.", true), pc("`bar` is next.", true)],
            want: vec![pc("uses `foo` properly.", true), pc("`bar` is next.", true)],
        },
        Case {
            name: "dangler alone is left untouched (unpaired opener renders literally)",
            input: vec![pc("opens `alpha-", true)],
            want: vec![pc("opens `alpha-", true)],
        },
        Case {
            name: "multiple spans, only last dangles: next paragraph repaired",
            input: vec![pc("`a` and `b", true), pc("c` d", true)],
            want: vec![pc("`a` and `b", true), pc("c\\` d", true)],
        },
        Case {
            name: "state disarms after one paragraph without a closer",
            input: vec![
                pc("opens `alpha-", true),
                pc("continuation without code.", true),
                pc("stray` here.", true),
            ],
            want: vec![
                pc("opens `alpha-", true),
                pc("continuation without code.", true),
                pc("stray` here.", true),
            ],
        },
        Case {
            name: "mismatched run is skipped; whitespace then disarms",
            input: vec![pc("opens `alpha-", true), pc("beta`` rest", true)],
            want: vec![pc("opens `alpha-", true), pc("beta`` rest", true)],
        },
        Case {
            name: "lone backtick inside a double-backtick split is skipped, closer repaired",
            input: vec![
                pc("opens ``alpha-", true),
                pc("be`ta`` rest ``x`` end", true),
            ],
            want: vec![
                pc("opens ``alpha-", true),
                pc("be`ta\\`\\` rest ``x`` end", true),
            ],
        },
        Case {
            name: "double backticks inside a single-backtick split are skipped, closer repaired",
            input: vec![pc("opens `alpha-", true), pc("be``ta` rest", true)],
            want: vec![pc("opens `alpha-", true), pc("be``ta\\` rest", true)],
        },
        Case {
            name: "double-backtick split repaired with matching run",
            input: vec![
                pc("opens ``alpha-", true),
                pc("beta`` rest ``x`` end", true),
            ],
            want: vec![
                pc("opens ``alpha-", true),
                pc("beta\\`\\` rest ``x`` end", true),
            ],
        },
        Case {
            name: "whitespace before the run disarms (closer must be in first word)",
            input: vec![pc("opens `alpha-", true), pc("two words` here", true)],
            want: vec![pc("opens `alpha-", true), pc("two words` here", true)],
        },
        Case {
            name: "closer at offset zero followed by whitespace repaired",
            input: vec![pc("opens `alpha-", true), pc("` for the rest", true)],
            want: vec![pc("opens `alpha-", true), pc("\\` for the rest", true)],
        },
        Case {
            name: "run at offset zero followed by a word stays a legitimate span",
            input: vec![pc("opens `alpha-", true), pc("`foo` bar", true)],
            want: vec![pc("opens `alpha-", true), pc("`foo` bar", true)],
        },
        Case {
            name: "fenced code between paragraphs disarms",
            input: vec![
                pc("opens `alpha-", true),
                fence_start(),
                Event::fenced_code_line("let x = 1;"),
                Event::fenced_code_end("```"),
                pc("beta` rest", true),
            ],
            want: vec![
                pc("opens `alpha-", true),
                fence_start(),
                Event::fenced_code_line("let x = 1;"),
                Event::fenced_code_end("```"),
                pc("beta` rest", true),
            ],
        },
        Case {
            name: "flush carrying the following paragraph is repaired",
            input: vec![pc("opens `alpha-", true), Event::flush("beta` tail")],
            want: vec![pc("opens `alpha-", true), Event::flush("beta\\` tail")],
        },
        Case {
            name: "flush is a region boundary: it does not arm",
            input: vec![Event::flush("opens `alpha-"), pc("beta` rest", true)],
            want: vec![Event::flush("opens `alpha-"), pc("beta` rest", true)],
        },
        Case {
            name: "closer prefix split across chunks still repaired",
            input: vec![
                pc("opens `alpha-", true),
                pc("num", false),
                pc("ber` rest", true),
            ],
            want: vec![
                pc("opens `alpha-", true),
                pc("num", false),
                pc("ber\\` rest", true),
            ],
        },
        Case {
            name: "non-streamed blocks participate on both sides",
            input: vec![
                Event::block("opens `alpha-"),
                Event::block("beta` rest `code` x"),
            ],
            want: vec![
                Event::block("opens `alpha-"),
                Event::block("beta\\` rest `code` x"),
            ],
        },
        Case {
            name: "chained danglers: repaired paragraph can itself dangle",
            input: vec![
                Event::block("see `alpha-"),
                Event::block("beta` mid `gamma-"),
                Event::block("delta` end"),
            ],
            want: vec![
                Event::block("see `alpha-"),
                Event::block("beta\\` mid `gamma-"),
                Event::block("delta\\` end"),
            ],
        },
        Case {
            name: "prefix longer than the cap disarms",
            input: vec![
                pc("opens `alpha-", true),
                pc(format!("{}` rest", "x".repeat(70)), true),
            ],
            want: vec![
                pc("opens `alpha-", true),
                pc(format!("{}` rest", "x".repeat(70)), true),
            ],
        },
        Case {
            name: "escaped backtick is literal: does not arm",
            input: vec![
                pc("literal \\` backtick here", true),
                pc("next` para", true),
            ],
            want: vec![
                pc("literal \\` backtick here", true),
                pc("next` para", true),
            ],
        },
        Case {
            name: "header-like block disarms via its whitespace",
            input: vec![pc("opens `alpha-", true), Event::block("# A `title`")],
            want: vec![pc("opens `alpha-", true), Event::block("# A `title`")],
        },
        Case {
            name: "terminal newline after closer does not confuse the repair",
            input: vec![pc("opens `alpha-", true), pc("omega`\n", true)],
            want: vec![pc("opens `alpha-", true), pc("omega\\`\n", true)],
        },
    ];

    for case in cases {
        let mut fixup = SplitCodeSpanFixup::new();
        let got: Vec<Event> = case
            .input
            .into_iter()
            .filter_map(|event| fixup.process(event))
            .collect();
        assert_eq!(got, case.want, "case: {}", case.name);
    }
}

/// The real-world quirk pushed through the full `Buffer` + `llm_quirks()`
/// pipeline: the orphaned closer is escaped and the later spans in the same
/// paragraph pair correctly again.
#[test]
fn split_code_span_through_buffer_pipeline() {
    let input = "delegate to `_rfd-next-draft-slot` for drafts and `_rfd-next-\n\nnumber` for \
                 permanent RFDs. I'm also extracting `_rfd-priority-rewrite` as a helper.\n\n";

    let mut buf = Buffer::new();
    buf.push(input);
    let mut fixups = Fixups::llm_quirks();
    let mut events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();
    events.extend(
        buf.flush_events()
            .into_iter()
            .filter_map(|event| fixups.apply(event)),
    );

    // Reassemble paragraph sources from streamed chunks and blocks.
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    for event in &events {
        match event {
            Event::ParagraphChunk { content, last, .. } => {
                current.push_str(content);
                if *last {
                    paragraphs.push(std::mem::take(&mut current));
                }
            }
            Event::Block { content, .. } => paragraphs.push(content.clone()),
            _ => {}
        }
    }

    assert_eq!(paragraphs.len(), 2, "events: {events:#?}");
    assert!(
        paragraphs[0].contains("`_rfd-next-"),
        "dangling opener must pass through untouched: {:?}",
        paragraphs[0]
    );
    assert!(
        paragraphs[1].starts_with("number\\`"),
        "orphaned closer must be escaped: {:?}",
        paragraphs[1]
    );
    assert!(
        paragraphs[1].contains("`_rfd-priority-rewrite`"),
        "later spans must pair correctly again: {:?}",
        paragraphs[1]
    );
}

/// The split-paragraph quirk when the stream ends without a trailing blank
/// line: the following paragraph reaches the fixup either as a terminal
/// [`Event::ParagraphChunk`] (if it began streaming) or as an [`Event::Flush`]
/// (if it did not), and the orphaned closer must be repaired on both paths.
#[test]
fn split_code_span_repaired_at_end_of_stream_without_blank_line() {
    let input = "opens `_rfd-next-\n\nnumber` tail";

    let mut buf = Buffer::new();
    buf.push(input);
    let mut fixups = Fixups::llm_quirks();
    let mut events: Vec<Event> = buf
        .by_ref()
        .filter_map(|event| fixups.apply(event))
        .collect();
    events.extend(
        buf.flush_events()
            .into_iter()
            .filter_map(|event| fixups.apply(event)),
    );

    let repaired = events.iter().any(|event| match event {
        Event::ParagraphChunk { content, .. }
        | Event::Block { content, .. }
        | Event::Flush { content, .. } => content.contains("number\\`"),
        _ => false,
    });
    assert!(
        repaired,
        "orphaned closer must be escaped even without a trailing blank line, events: {events:#?}"
    );
}

#[test]
fn paragraph_chunk_embedded_fence_split_before_fence_still_detected() {
    // The inline scanner holds an embedded fence run intact, committing the
    // prose before it in an earlier chunk and landing the fence at the START of
    // a later chunk. A per-chunk line check would skip a chunk beginning with
    // backticks; the fixup must detect the fence over the whole accumulated
    // paragraph so the following bare fence is still suppressed.
    let mut fixup = OrphanedFenceFixup::new();
    for chunk in [
        Event::paragraph_chunk("Let me run this command for you now: ", false),
        Event::paragraph_chunk("```rust", false),
        Event::paragraph_chunk("\n", true),
    ] {
        assert!(matches!(
            fixup.process(chunk),
            Some(Event::ParagraphChunk { .. })
        ));
    }

    let converted = fixup.process(Event::FencedCodeStart {
        language: String::new(),
        fence_type: FenceType::Backtick,
        fence_length: 3,
        indent: 0,
    });
    assert!(
        matches!(converted, Some(Event::Block { .. })),
        "an embedded fence split before the backticks must still be detected, got: {converted:?}"
    );
}
