use super::*;

#[test]
fn hyperlink_wraps_the_text_in_an_osc_8_link() {
    assert_eq!(
        hyperlink("jp://show-events/abc", "abc"),
        "\x1b]8;;jp://show-events/abc\x07abc\x1b]8;;\x07"
    );
}

#[test]
fn a_uri_cannot_end_the_link_early() {
    // A BEL, or the ESC of an ST, would end the sequence inside the URI, and
    // the rest of the URI would reach the terminal as raw input.
    assert_eq!(
        hyperlink("file:///tmp/a\x07\x1b]2;pwned\x1b\\b", "open"),
        "\x1b]8;;file:///tmp/a]2;pwned\\b\x07open\x1b]8;;\x07"
    );
}

#[test]
fn every_control_character_is_removed_from_a_uri() {
    // C0 (a line feed, a tab), DEL, and C1. A terminal that reads 8-bit
    // controls takes U+009C as ST and U+009D as the start of an OSC.
    assert_eq!(
        hyperlink("a\nb\tc\x7fd\u{9c}e\u{9d}f", "x"),
        "\x1b]8;;abcdef\x07x\x1b]8;;\x07"
    );
}

#[test]
fn the_link_text_keeps_its_styling() {
    // The text is displayed between the two sequences rather than embedded in
    // one, and callers color it.
    assert_eq!(
        hyperlink("file:///tmp/a", "\x1b[31mopen\x1b[39m"),
        "\x1b]8;;file:///tmp/a\x07\x1b[31mopen\x1b[39m\x1b]8;;\x07"
    );
}

#[test]
fn a_title_cannot_end_the_title_sequence_early() {
    assert_eq!(
        title_sequence("fix\x07\x1b[2J bug\x1b\\"),
        "\x1b]2;fix[2J bug\\\x07"
    );
}

#[test]
fn every_control_character_is_removed_from_a_title() {
    assert_eq!(
        title_sequence("a\nb\tc\x7fd\u{9c}e\u{9d}f"),
        "\x1b]2;abcdef\x07"
    );
}

#[test]
fn a_title_keeps_everything_but_control_characters() {
    assert_eq!(
        title_sequence("jp-abc: Fix the 🦀 build"),
        "\x1b]2;jp-abc: Fix the 🦀 build\x07"
    );
}
