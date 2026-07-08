use super::*;

#[test]
fn test_visual_width_plain() {
    assert_eq!(visual_width("hello"), 5);
    assert_eq!(visual_width(""), 0);
    assert_eq!(visual_width("a b c"), 5);
}

#[test]
fn test_visual_width_wide_chars() {
    // Emoji: ✅ is a wide character (2 columns).
    assert_eq!(visual_width("✅"), 2);
    // CJK ideograph: 漢 is 2 columns.
    assert_eq!(visual_width("漢字"), 4);
    // Mixed: ASCII + emoji.
    assert_eq!(visual_width("ok ✅"), 5);
    // Wide char inside ANSI escapes.
    assert_eq!(visual_width("\x1b[1m✅\x1b[22m"), 2);
}

#[test]
fn test_visual_width_vs16_emoji() {
    // U+26A0 WARNING SIGN alone is width 1.
    assert_eq!(visual_width("\u{26A0}"), 1);
    // U+26A0 + U+FE0F (VS16) forces emoji presentation = width 2.
    assert_eq!(visual_width("\u{26A0}\u{FE0F}"), 2);
    // Mixed with text: 2 + 1 + 7 = 10.
    assert_eq!(visual_width("\u{26A0}\u{FE0F} warning"), 10);
    // Multiple VS16 emoji in a string.
    assert_eq!(visual_width("\u{26A0}\u{FE0F} ok \u{26A0}\u{FE0F}"), 8);
    // VS16 after an already-wide character is a no-op.
    assert_eq!(visual_width("\u{2705}\u{FE0F}"), 2);
    // VS16 inside ANSI escapes.
    assert_eq!(visual_width("\x1b[1m\u{26A0}\u{FE0F}\x1b[22m"), 2);
}

/// An ANSI escape between a base character and its VS16 must not change the
/// measured width: `visual_width` ignores escapes, so the visible text is the
/// contiguous glyph U+26A0 U+FE0F (width 2), not a width-1 warning sign plus a
/// width-0 VS16 measured in isolation.
#[test]
fn test_visual_width_escape_splitting_vs16_glyph() {
    assert_eq!(visual_width("\u{26A0}\x1b[1m\u{FE0F}\x1b[22m"), 2);
}

#[test]
fn test_visual_width_zwj_sequences() {
    // ZWJ family emoji: multiple emoji joined into a single glyph.
    // U+1F468 + U+200D + U+1F469 + U+200D + U+1F467
    assert_eq!(
        visual_width("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
        2
    );
    // Woman scientist: U+1F469 + U+200D + U+1F52C
    assert_eq!(visual_width("\u{1F469}\u{200D}\u{1F52C}"), 2);
}

#[test]
fn test_visual_width_with_ansi() {
    assert_eq!(visual_width("\x1b[1mbold\x1b[22m"), 4);
    assert_eq!(visual_width("\x1b[48;5;248m`code`\x1b[49m"), 6);
    assert_eq!(
        visual_width("\x1b[1m**Hello**\x1b[22m \x1b[3m*World*\x1b[23m"),
        17
    );
}

#[test]
fn test_state_update_toggle() {
    let mut s = AnsiState::default();
    assert!(!s.is_active());

    s.update(BOLD_START);
    assert!(s.bold);
    assert!(s.is_active());

    s.update(BOLD_END);
    assert!(!s.bold);
    assert!(!s.is_active());
}

#[test]
fn test_state_update_colors() {
    let mut s = AnsiState::default();
    s.update("\x1b[48;5;248m");
    assert_eq!(s.background.as_deref(), Some("48;5;248"));

    s.update("\x1b[38;5;248m");
    assert_eq!(s.foreground.as_deref(), Some("38;5;248"));

    s.update(BG_END);
    assert!(s.background.is_none());

    s.update(FG_END);
    assert!(s.foreground.is_none());
}

#[test]
fn test_state_reset_clears_all() {
    let mut s = AnsiState {
        bold: true,
        italic: true,
        background: Some("48;5;248".into()),
        foreground: Some("38;5;248".into()),
        ..Default::default()
    };

    s.update(RESET);
    assert!(!s.is_active());
}

#[test]
fn test_state_update_from_str() {
    let mut s = AnsiState::default();
    s.update_from_str("\x1b[1m**hello**\x1b[22m \x1b[3m*world*\x1b[23m");
    // After the full string both bold and italic have been toggled on then off,
    // so nothing should be active.
    assert!(!s.is_active());
}

#[test]
fn test_state_update_from_str_partial() {
    let mut s = AnsiState::default();
    // Bold opened but never closed.
    s.update_from_str("\x1b[1m**hello");
    assert!(s.bold);
    assert!(s.is_active());
}

#[test]
fn test_restore_sequence_roundtrip() {
    let s = AnsiState {
        bold: true,
        italic: true,
        background: Some("48;5;248".into()),
        foreground: Some("38;5;100".into()),
        ..Default::default()
    };

    let seq = s.restore_sequence();
    assert!(seq.contains(BOLD_START));
    assert!(seq.contains(ITALIC_START));
    assert!(seq.contains("48;5;248"));
    assert!(seq.contains("38;5;100"));
}

#[test]
fn compound_sgr_tracks_every_sub_parameter() {
    // A single escape combining a style attribute with a background: matching
    // only the leading sub-parameter would miss the background entirely.
    let mut s = AnsiState::default();
    assert!(s.update("\x1b[1;48;5;236m"));
    assert!(s.bold);
    assert_eq!(s.background.as_deref(), Some("48;5;236"));
}

#[test]
fn compound_sgr_leading_reset_clears_prior_state() {
    let mut s = AnsiState {
        bold: true,
        ..Default::default()
    };
    assert!(s.update("\x1b[0;48;5;236m"));
    assert!(!s.bold, "the leading 0 reset the prior bold");
    assert_eq!(s.background.as_deref(), Some("48;5;236"));
}

#[test]
fn compound_sgr_clears_foreground_and_background_together() {
    let mut s = AnsiState {
        foreground: Some("38;5;1".into()),
        background: Some("48;5;1".into()),
        ..Default::default()
    };
    assert!(s.update("\x1b[39;49m"));
    assert!(s.foreground.is_none());
    assert!(s.background.is_none());
}

#[test]
fn compound_truecolor_foreground_and_background() {
    // The 24-bit color operands must not be misread as further attributes.
    let mut s = AnsiState::default();
    s.update("\x1b[38;2;1;2;3;48;2;4;5;6m");
    assert_eq!(s.foreground.as_deref(), Some("38;2;1;2;3"));
    assert_eq!(s.background.as_deref(), Some("48;2;4;5;6"));
}

#[test]
fn update_reports_background_events() {
    let mut s = AnsiState::default();
    assert!(
        !s.update(BOLD_START),
        "a style attribute is not a background event"
    );
    assert!(
        s.update("\x1b[48;5;236m"),
        "setting a background is an event"
    );
    assert!(s.update(BG_END), "clearing a background is an event");
    assert!(s.update(RESET), "a reset is an event");
    assert!(
        !s.update("\x1b[38;5;200m"),
        "setting a foreground is not a background event"
    );
    assert!(
        !s.update(FG_END),
        "clearing a foreground is not a background event"
    );
    assert!(
        !s.update("\x1b[K"),
        "a non-SGR escape is not a background event"
    );
}

#[test]
fn segments_keeps_an_st_terminated_osc_sequence_whole() {
    // An OSC 8 hyperlink: the open/close sequences must each tokenize as one
    // escape rather than splitting at the first letter of the URL.
    let parts: Vec<_> = segments("\x1b]8;;url\x1b\\link\x1b]8;;\x1b\\").collect();
    assert_eq!(parts, vec![
        Segment::Escape("\x1b]8;;url\x1b\\"),
        Segment::Text("link"),
        Segment::Escape("\x1b]8;;\x1b\\"),
    ]);
}

#[test]
fn segments_keeps_a_bel_terminated_osc_sequence_whole() {
    let parts: Vec<_> = segments("\x1b]8;;url\x07text").collect();
    assert_eq!(parts, vec![
        Segment::Escape("\x1b]8;;url\x07"),
        Segment::Text("text"),
    ]);
}

#[test]
fn segments_yields_an_unterminated_osc_as_one_escape() {
    // No terminator yet (split across a write boundary): the whole remainder is
    // a single, incomplete escape.
    let parts: Vec<_> = segments("\x1b]8;;url").collect();
    assert_eq!(parts, vec![Segment::Escape("\x1b]8;;url")]);
}

#[test]
fn simple_and_bright_background_codes_are_tracked() {
    // `\x1b[41m` is crossterm's `on_red()`; a writer that misses it would
    // shade over a background the content owns.
    let mut s = AnsiState::default();
    assert!(s.update("\x1b[41m"), "a simple background set is an event");
    assert_eq!(s.background.as_deref(), Some("41"));
    assert!(s.update("\x1b[49m"));
    assert!(s.update("\x1b[101m"), "a bright background set is an event");
    assert_eq!(s.background.as_deref(), Some("101"));
}

#[test]
fn simple_and_bright_foreground_codes_are_tracked() {
    let mut s = AnsiState::default();
    assert!(
        !s.update("\x1b[31m"),
        "a foreground set is not a background event"
    );
    assert_eq!(s.foreground.as_deref(), Some("31"));
    assert!(!s.update("\x1b[97m"));
    assert_eq!(s.foreground.as_deref(), Some("97"));
}

#[test]
fn visual_width_ignores_an_osc_hyperlink() {
    // The hyperlink chrome is zero-width; only the link text counts.
    assert_eq!(
        visual_width("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"),
        4
    );
}
