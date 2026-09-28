use std::fmt::Write as _;

use super::*;

/// Run `chunks` through one writer as separate writes, then finish it.
fn filter(class: ContentClass, mode: SanitizeMode, chunks: &[&str]) -> String {
    let mut out = String::new();
    {
        let mut writer = ContentWriter::new(&mut out, class, mode);
        for chunk in chunks {
            writer.write_str(chunk).unwrap();
        }
        writer.finish().unwrap();
    }
    out
}

/// Tool output under `strip`, written in one piece.
fn tool(content: &str) -> String {
    filter(ContentClass::ToolOutput, SanitizeMode::Strip, &[content])
}

/// Model output under `strip`, written in one piece.
fn model(content: &str) -> String {
    filter(ContentClass::ModelOutput, SanitizeMode::Strip, &[content])
}

/// Model output under `visualize`, written in one piece.
fn visualized(content: &str) -> String {
    filter(ContentClass::ModelOutput, SanitizeMode::Visualize, &[
        content,
    ])
}

/// Records every write it receives, so a test can see when output arrived.
#[derive(Default)]
struct Writes(Vec<String>);

impl fmt::Write for Writes {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.push(s.to_owned());
        Ok(())
    }
}

#[test]
fn text_line_feeds_and_tabs_pass_through() {
    assert_eq!(model("plain text\n\tindented"), "plain text\n\tindented");
}

#[test]
fn tool_output_keeps_its_styling() {
    // A colored diff from a user-built tool should look like the diff.
    assert_eq!(
        tool("\x1b[31m-old\x1b[0m\n\x1b[1;32m+new\x1b[0m"),
        "\x1b[31m-old\x1b[0m\n\x1b[1;32m+new\x1b[0m\x1b[0m"
    );
}

#[test]
fn model_output_drops_its_styling() {
    assert_eq!(model("\x1b[31mred\x1b[0m text"), "red text");
}

#[test]
fn sequences_that_move_the_cursor_or_erase_are_dropped() {
    // Cursor movement can rewrite a line the user already read; an erase or a
    // scroll can hide one.
    assert_eq!(
        tool("a\x1b[2Jb\x1b[1;1Hc\x1b[3Ad\x1b[Ke\x1b[5Sf"),
        "abcdef\x1b[0m"
    );
}

#[test]
fn mode_changes_are_dropped() {
    // The alternate screen, bracketed paste, and mouse reporting all break the
    // prompts that follow. `m` behind a private marker sets a keyboard mode
    // rather than styling, so even tool output loses it.
    assert_eq!(
        tool("a\x1b[?1049hb\x1b[?2004hc\x1b[?1000hd\x1b[>4;2me"),
        "abcde\x1b[0m"
    );
}

#[test]
fn osc_sequences_are_dropped_with_their_payload() {
    // A window title, a clipboard write, and a hyperlink, ended by BEL and by
    // ST. The hyperlink's text sits between its two sequences and survives.
    assert_eq!(model("\x1b]0;title\x07a"), "a");
    assert_eq!(model("\x1b]2;title\x1b\\a"), "a");
    assert_eq!(model("\x1b]52;c;ZXZpbA==\x07a"), "a");
    assert_eq!(
        model("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\"),
        "link"
    );
}

#[test]
fn string_sequences_are_dropped_with_their_payload() {
    // DCS (a settings query), APC (a kitty graphics image), PM, and SOS.
    assert_eq!(model("a\x1bP$qm\x1b\\b"), "ab");
    assert_eq!(model("a\x1b_Gf=100;iVBORw0KGgo=\x1b\\b"), "ab");
    assert_eq!(model("a\x1b^private\x1b\\b"), "ab");
    assert_eq!(model("a\x1bXstring\x1b\\b"), "ab");
}

#[test]
fn other_escape_sequences_are_dropped() {
    // `ESC ( 0` would draw the text after it as line-drawing glyphs; `ESC 7`
    // and `ESC 8` save and restore the cursor; `ESC c` resets the terminal.
    assert_eq!(model("\x1b(0qq\x1b(B"), "qq");
    assert_eq!(model("a\x1b7b\x1b8c\x1bcd"), "abcd");
}

#[test]
fn control_characters_are_dropped() {
    // A carriage return overwrites the line it returns to, the classic way to
    // spoof a log line; a backspace does the same one character at a time.
    assert_eq!(model("safe\rspoofed"), "safespoofed");
    assert_eq!(model("a\x08b"), "ab");
    assert_eq!(model("bell\x07"), "bell");
    assert_eq!(model("del\x7f"), "del");
}

#[test]
fn c1_controls_are_dropped() {
    // A terminal honoring C1 reads U+009B as CSI. Without it, what follows is
    // plain text.
    assert_eq!(model("a\u{9b}2Jb"), "a2Jb");
    assert_eq!(model("a\u{85}b"), "ab");
}

#[test]
fn conceal_is_removed_and_its_neighbours_kept() {
    // Text the reader cannot see goes, but the bold and the color stay.
    assert_eq!(tool("\x1b[1;8;31mx"), "\x1b[1;31mx\x1b[0m");
    assert_eq!(tool("\x1b[8mhidden"), "hidden\x1b[0m");
}

#[test]
fn a_color_operand_of_eight_is_not_conceal() {
    // `38`, `48`, and `58` carry their color after them, and an `8` there is a
    // color value.
    assert_eq!(tool("\x1b[38;5;8mx"), "\x1b[38;5;8mx\x1b[0m");
    assert_eq!(tool("\x1b[48;2;8;8;8mx"), "\x1b[48;2;8;8;8mx\x1b[0m");
    assert_eq!(tool("\x1b[58:5:8mx"), "\x1b[58:5:8mx\x1b[0m");
    assert_eq!(tool("\x1b[8;38;5;8mx"), "\x1b[38;5;8mx\x1b[0m");
}

#[test]
fn conceal_is_recognized_however_it_is_written() {
    // A terminal reads `08` as `8`, and one that ignores a sub-parameter it
    // has no use for reads `8:1` as `8` too.
    assert_eq!(tool("\x1b[08mx"), "x\x1b[0m");
    assert_eq!(tool("\x1b[8:1mx"), "x\x1b[0m");
}

#[test]
fn visualize_marks_each_dropped_sequence_once() {
    // A string's payload and terminator belong to the string, so the OSC, the
    // APC, and the DCS each earn one marker.
    assert_eq!(
        visualized("a\x1b[2Jb\x1b]0;title\x1b\\c\x1b_Gpayload\x1b\\d\x1bPq#0\x1b\\e"),
        "a\u{241b}b\u{241b}c\u{241b}d\u{241b}e"
    );
}

#[test]
fn visualize_marks_dropped_controls() {
    assert_eq!(
        visualized("safe\rspoofed\x07"),
        "safe\u{241b}spoofed\u{241b}"
    );
}

#[test]
fn visualize_marks_model_output_styling_as_dropped() {
    assert_eq!(visualized("\x1b[31mred\x1b[0m"), "\u{241b}red\u{241b}");
    // The whole sequence is dropped, so a conceal inside it adds no second
    // marker.
    assert_eq!(visualized("\x1b[1;8mx"), "\u{241b}x");
}

#[test]
fn visualize_marks_a_removed_conceal() {
    // The marker stands where the sequence was, and what survives of the
    // sequence follows it.
    assert_eq!(
        filter(ContentClass::ToolOutput, SanitizeMode::Visualize, &[
            "\x1b[1;8mx"
        ]),
        "\u{241b}\x1b[1mx\x1b[0m"
    );
}

#[test]
fn a_sequence_split_across_writes_is_recognized_whole() {
    // Streamed content splits wherever a chunk happens to end.
    assert_eq!(
        filter(ContentClass::ModelOutput, SanitizeMode::Strip, &[
            "a\x1b[", "2", "Jb"
        ]),
        "ab"
    );
    assert_eq!(
        filter(ContentClass::ToolOutput, SanitizeMode::Strip, &[
            "\x1b[3", "1mred"
        ]),
        "\x1b[31mred\x1b[0m"
    );
}

#[test]
fn an_escape_ending_a_write_starts_a_sequence_in_the_next() {
    assert_eq!(
        filter(ContentClass::ToolOutput, SanitizeMode::Strip, &[
            "a\x1b", "[31mb"
        ]),
        "a\x1b[31mb\x1b[0m"
    );
}

#[test]
fn an_osc_split_across_writes_keeps_its_payload_hidden() {
    assert_eq!(
        filter(ContentClass::ModelOutput, SanitizeMode::Strip, &[
            "\x1b]0;ti",
            "tle\x07after"
        ]),
        "after"
    );
    // The ST is split too, and its `\` still belongs to the string.
    assert_eq!(
        filter(ContentClass::ModelOutput, SanitizeMode::Visualize, &[
            "\x1b]0;title\x1b",
            "\\after"
        ]),
        "\u{241b}after"
    );
}

#[test]
fn text_reaches_the_wrapped_writer_before_the_write_returns() {
    // Only an unfinished sequence is held back, never the text before it.
    let mut writes = Writes::default();
    {
        let mut writer =
            ContentWriter::new(&mut writes, ContentClass::ToolOutput, SanitizeMode::Strip);
        writer.write_str("foo \x1b[3").unwrap();
        writer.write_str("1mbar").unwrap();
    }
    assert_eq!(writes.0, ["foo ", "\x1b[31mbar"]);
}

#[test]
fn finish_drops_an_unfinished_sequence() {
    assert_eq!(model("text\x1b[3"), "text");
    assert_eq!(visualized("text\x1b]0;tit"), "text\u{241b}");
}

#[test]
fn finish_closes_the_span_after_styling_left_open() {
    // A result truncated to its first lines loses the reset at its end; the
    // span's reset stops the color running into whatever is written next.
    assert_eq!(
        tool("\x1b[41mline 1\nline 2"),
        "\x1b[41mline 1\nline 2\x1b[0m"
    );
    // An unfinished sequence is dropped ahead of the reset.
    assert_eq!(tool("\x1b[31mred\x1b[2"), "\x1b[31mred\x1b[0m");
}

#[test]
fn off_passes_everything_through_and_still_closes_the_span() {
    assert_eq!(
        filter(ContentClass::ToolOutput, SanitizeMode::Off, &[
            "\x1b[31mred\x1b[2J\r\x1b]0;t\x07"
        ]),
        "\x1b[31mred\x1b[2J\r\x1b]0;t\x07\x1b[0m"
    );
}

#[test]
fn a_class_without_a_span_writes_no_reset() {
    assert_eq!(model("\x1b[31mred"), "red");
    assert_eq!(
        filter(ContentClass::ModelOutput, SanitizeMode::Off, &[
            "\x1b[31mred"
        ]),
        "\x1b[31mred"
    );
}

#[test]
fn an_unfinished_sequence_does_not_join_content_written_after_finish() {
    // Reasoning and message text interleave within a turn, so a writer
    // finished at one transition takes content again after the next.
    let mut out = String::new();
    {
        let mut writer =
            ContentWriter::new(&mut out, ContentClass::ModelOutput, SanitizeMode::Strip);
        writer.write_str("a\x1b[3").unwrap();
        writer.finish().unwrap();
        writer.write_str("1mb").unwrap();
        writer.finish().unwrap();
    }
    assert_eq!(out, "a1mb");
}

#[test]
fn visible_sgr_removes_conceal() {
    assert_eq!(visible_sgr("\x1b[1;8;31m").as_deref(), Some("\x1b[1;31m"));
    assert_eq!(visible_sgr("\x1b[8m"), None);
}

#[test]
fn visible_sgr_rejects_everything_but_sgr() {
    assert_eq!(visible_sgr("\x1b[2J"), None);
    assert_eq!(visible_sgr("\x1b[>4;2m"), None);
    assert_eq!(visible_sgr("\x1b]0;title\x07"), None);
}

#[test]
fn visible_sgr_rebuilds_the_sequence_from_its_parameters() {
    assert_eq!(visible_sgr("\x1b[m").as_deref(), Some("\x1b[0m"));
    assert_eq!(visible_sgr("\x1b[01;031m").as_deref(), Some("\x1b[1;31m"));
}
