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
