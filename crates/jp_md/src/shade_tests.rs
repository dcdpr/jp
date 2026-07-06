use std::fmt::Write as _;

use super::*;

/// A full-width (`\x1b[K`) reasoning-style background.
fn terminal_bg() -> DefaultBackground {
    DefaultBackground {
        param: "48;5;236".into(),
        fill: BackgroundFill::Terminal,
    }
}

#[test]
fn fills_each_line_to_the_edge_and_ends_the_region() {
    // The background is asserted once and persists across the newline; each
    // line is erased to the right edge, and the region is closed with `\x1b[49m`.
    assert_eq!(
        shade("a\nb\n", &terminal_bg()),
        "\x1b[48;5;236ma\x1b[K\nb\x1b[K\n\x1b[49m"
    );
}

#[test]
fn empty_input_produces_no_output() {
    assert_eq!(shade("", &terminal_bg()), "");
}

#[test]
fn content_background_is_preserved_and_the_region_resumes_after_it() {
    // While the content owns the background the writer stays out of the way;
    // once the content clears it (`\x1b[49m`) the region background resumes.
    assert_eq!(
        shade("a\x1b[48;5;52mb\x1b[49mc", &terminal_bg()),
        "\x1b[48;5;236ma\x1b[48;5;52mb\x1b[49m\x1b[48;5;236mc\x1b[49m"
    );
}

#[test]
fn compound_sgr_background_is_recognized() {
    // The content's background is set in a compound escape (`\x1b[1;48;5;52m`);
    // the writer must see it and not shade over it, then resume after the reset.
    assert_eq!(
        shade("\x1b[1;48;5;52mx\x1b[0my", &terminal_bg()),
        "\x1b[1;48;5;52mx\x1b[0m\x1b[48;5;236my\x1b[49m"
    );
}

#[test]
fn carriage_return_rewrite_keeps_the_background_active() {
    // A `\r\x1b[K` rewrite (the temp/progress line) erases and redraws on the
    // region background, which persists across the carriage return.
    assert_eq!(
        shade("foo\r\x1b[Kbar", &terminal_bg()),
        "\x1b[48;5;236mfoo\r\x1b[Kbar\x1b[49m"
    );
}

#[test]
fn erase_under_a_content_background_keeps_the_content_fill() {
    // When the content has its own background, its `\x1b[K` erase must fill with
    // that background — the region background is never injected before it.
    let output = shade("\x1b[48;5;52m\x1b[Kx", &terminal_bg());
    assert_eq!(output, "\x1b[48;5;52m\x1b[Kx\x1b[49m");
    assert!(
        !output.contains("\x1b[48;5;236m"),
        "region background must not be injected over a content background: {output:?}"
    );
}

#[test]
fn a_reset_mid_stream_re_asserts_the_region_background() {
    assert_eq!(
        shade("a\x1b[0mb", &terminal_bg()),
        "\x1b[48;5;236ma\x1b[0m\x1b[48;5;236mb\x1b[49m"
    );
}

#[test]
fn non_sgr_non_erase_escapes_pass_through_verbatim() {
    // A cursor move (`\x1b[2A`) is neither SGR nor a CSI erase, so it flows
    // through unchanged and does not disturb the background.
    let output = shade("a\x1b[2Ab", &terminal_bg());
    assert_eq!(output, "\x1b[48;5;236ma\x1b[2Ab\x1b[49m");
}

#[test]
fn content_fill_mode_omits_the_edge_erase() {
    // A non-`Terminal` fill backs only the content, so no `\x1b[K` is emitted.
    let content_bg = DefaultBackground {
        param: "48;5;236".into(),
        fill: BackgroundFill::Content,
    };
    let output = shade("a\nb", &content_bg);
    assert_eq!(output, "\x1b[48;5;236ma\nb\x1b[49m");
    assert!(
        !output.contains("\x1b[K"),
        "content fill must not erase: {output:?}"
    );
}

#[test]
fn an_escape_split_across_writes_is_reassembled() {
    // The background-setting escape is cut between two writes; the writer holds
    // the partial sequence and completes it on the next write.
    let mut buffer = String::new();
    {
        let mut writer = ShadedWriter::new(&mut buffer, &terminal_bg());
        writer.write_str("a\x1b[4").unwrap();
        writer.write_str("8;5;52mb").unwrap();
        writer.finish().unwrap();
    }
    assert_eq!(buffer, "\x1b[48;5;236ma\x1b[48;5;52mb\x1b[49m");
}

#[test]
fn a_reset_then_carriage_return_re_asserts_before_the_erase() {
    // After a content reset the region background is owed; a following
    // `\r\x1b[K` must re-assert it so the erase fills with the region color.
    assert_eq!(
        shade("foo\x1b[0m\r\x1b[Kbar", &terminal_bg()),
        "\x1b[48;5;236mfoo\x1b[0m\r\x1b[48;5;236m\x1b[Kbar\x1b[49m"
    );
}

#[test]
fn osc_hyperlink_passes_through_intact_and_shades_the_link_text() {
    // The OSC 8 open/close sequences flow through verbatim (the URL is never
    // split or shaded over), while the visible link text gets the region
    // background.
    assert_eq!(
        shade("\x1b]8;;url\x1b\\link\x1b]8;;\x1b\\", &terminal_bg()),
        "\x1b]8;;url\x1b\\\x1b[48;5;236mlink\x1b]8;;\x1b\\\x1b[49m"
    );
}

#[test]
fn osc_hyperlink_split_across_writes_is_reassembled() {
    // The OSC sequence is cut mid-URL between two writes; the writer holds the
    // partial sequence (OSC terminates on BEL/ST, not a letter) and completes
    // it before forwarding.
    let mut buffer = String::new();
    {
        let mut writer = ShadedWriter::new(&mut buffer, &terminal_bg());
        writer.write_str("\x1b]8;;ur").unwrap();
        writer.write_str("l\x1b\\x").unwrap();
        writer.finish().unwrap();
    }
    assert_eq!(buffer, "\x1b]8;;url\x1b\\\x1b[48;5;236mx\x1b[49m");
}
