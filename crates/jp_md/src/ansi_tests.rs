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
