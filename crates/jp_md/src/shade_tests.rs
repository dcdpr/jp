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
fn fills_each_line_to_the_edge_and_closes_before_the_break() {
    // Each line is erased to the right edge, then the background is closed
    // before the newline and re-asserted on the next line.
    //
    // The close has to precede the break: a terminal scrolling to make room for
    // the next row fills that row with the background active at the time (the
    // `bce` capability), so a `\n` written under the region paints a row the
    // region does not own.
    assert_eq!(
        shade("a\nb\n", &terminal_bg()),
        "\x1b[48;5;236ma\x1b[K\x1b[49m\n\x1b[48;5;236mb\x1b[K\x1b[49m\n"
    );
}

#[test]
fn empty_input_produces_no_output() {
    assert_eq!(shade("", &terminal_bg()), "");
}

/// A background that pads to a fixed column instead of deferring to the
/// terminal.
fn column_bg(width: usize) -> DefaultBackground {
    DefaultBackground {
        param: "48;5;236".into(),
        fill: BackgroundFill::Column(width),
    }
}

#[test]
fn column_fill_pads_with_spaces_instead_of_erasing() {
    // A host that lays out its own sub-window (an `fzf` preview pane) drops
    // `\x1b[K`, so the fill has to be real characters. Each line is padded from
    // its own visible width up to the column.
    assert_eq!(
        shade("ab\nc\n", &column_bg(4)),
        "\x1b[48;5;236mab  \x1b[49m\n\x1b[48;5;236mc   \x1b[49m\n"
    );
}

#[test]
fn column_fill_counts_display_width_not_bytes() {
    // Two double-width characters fill four of the six columns, so two spaces
    // remain. Counting bytes would have padded nothing (6 bytes >= 6).
    assert_eq!(
        shade("日本\n", &column_bg(6)),
        "\x1b[48;5;236m日本  \x1b[49m\n"
    );
}

#[test]
fn column_fill_ignores_escape_bytes_when_measuring() {
    // The content's own styling is zero-width: `ab` is still 2 columns, so the
    // pad is 2, not 2-minus-the-escape-length.
    let shaded = shade("a\x1b[1mb\x1b[22m\n", &column_bg(4));
    assert!(
        shaded.ends_with("  \x1b[49m\n"),
        "expected a two-space pad, got {shaded:?}"
    );
}

#[test]
fn column_fill_moves_a_tab_to_the_next_tab_stop() {
    // Tool output is full of tabs. `unicode_width` measures one column each,
    // while a terminal and an `fzf` pane both move the cursor to the next
    // multiple of 8, so the plain measurement pads past the target column.
    assert_eq!(
        shade("a\tb\n", &column_bg(16)),
        "\x1b[48;5;236ma\tb       \x1b[49m\n",
        "the tab lands on column 8, so `b` ends at 9 and 7 columns remain"
    );
}

#[test]
fn column_fill_emits_nothing_once_the_line_reaches_the_column() {
    // An over-long line gets no padding rather than a negative one.
    assert_eq!(
        shade("abcdef\n", &column_bg(4)),
        "\x1b[48;5;236mabcdef\x1b[49m\n"
    );
}

#[test]
fn content_fill_backs_only_the_text() {
    // `Content` asserts the background under the text but never extends it.
    let bg = DefaultBackground {
        param: "48;5;236".into(),
        fill: BackgroundFill::Content,
    };
    assert_eq!(shade("ab\n", &bg), "\x1b[48;5;236mab\x1b[49m\n");
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
    assert_eq!(output, "\x1b[48;5;236ma\x1b[49m\n\x1b[48;5;236mb\x1b[49m");
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
fn simple_background_code_is_preserved() {
    // `\x1b[41m` (crossterm's `on_red()`) sets a content background with a
    // simple SGR code, not the extended `48;…` form; the writer must not
    // inject the region fill over it, and must resume the region once the
    // content clears its background.
    assert_eq!(
        shade("\x1b[41mred\x1b[49mplain", &terminal_bg()),
        "\x1b[41mred\x1b[49m\x1b[48;5;236mplain\x1b[49m"
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
