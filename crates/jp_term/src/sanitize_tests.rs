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
fn user_messages_keep_their_styling_and_leave_closing_it_to_the_renderer() {
    // The span of a user message closes on its formatted output: a reset in the
    // markdown source could change how the message parses.
    assert_eq!(
        filter(ContentClass::UserMessage, SanitizeMode::Strip, &[
            "\x1b[31mred\x1b[2J"
        ]),
        "\x1b[31mred"
    );
}

#[test]
fn user_messages_lose_everything_but_styling() {
    assert_eq!(
        filter(ContentClass::UserMessage, SanitizeMode::Strip, &[
            "a\rb\x1b]0;title\x07c\x1b[1Ad\x1b[8me"
        ]),
        "abcde"
    );
}

#[test]
fn get_mut_hands_over_what_each_write_produced() {
    // A caller filtering a stream takes each write's output as it goes, while a
    // sequence the write cut short stays held for the next one.
    let mut writer = ContentWriter::new(
        String::new(),
        ContentClass::ModelOutput,
        SanitizeMode::Strip,
    );

    writer.write_str("abc\x1b[3").unwrap();
    assert_eq!(std::mem::take(writer.get_mut()), "abc");

    writer.write_str("1mdef").unwrap();
    assert_eq!(std::mem::take(writer.get_mut()), "def");
}

#[test]
fn sanitize_str_filters_text_in_one_piece() {
    assert_eq!(
        sanitize_str(
            "a\x1b[31mb\x1b[",
            ContentClass::ModelOutput,
            SanitizeMode::Strip
        ),
        "ab"
    );
    assert_eq!(
        sanitize_str(
            "a\x1b[31mb\x1b[",
            ContentClass::ToolOutput,
            SanitizeMode::Strip
        ),
        "a\x1b[31mb\x1b[0m"
    );
    assert_eq!(
        sanitize_str(
            "a\x1b[2J",
            ContentClass::ModelOutput,
            SanitizeMode::Visualize
        ),
        "a\u{241b}"
    );
}

#[test]
fn decoded_model_output_loses_its_control_characters() {
    // `&#27;[2J` decodes to a real `ESC` after the source filter has run. Only
    // the control character goes; the rest was ordinary text all along.
    assert_eq!(
        sanitize_decoded(
            "clear \x1b[2J, return\r, bell\x07, del\x7f, csi\u{9b}",
            ContentClass::ModelOutput,
            SanitizeMode::Strip
        ),
        "clear [2J, return, bell, del, csi"
    );
}

#[test]
fn decoded_model_output_keeps_line_feeds_and_tabs() {
    assert_eq!(
        sanitize_decoded("a\nb\tc", ContentClass::ModelOutput, SanitizeMode::Strip),
        "a\nb\tc"
    );
}

#[test]
fn visualize_marks_each_control_character_decoded_model_output_lost() {
    assert_eq!(
        sanitize_decoded(
            "clear \x1b[2J\x07",
            ContentClass::ModelOutput,
            SanitizeMode::Visualize
        ),
        "clear \u{241b}[2J\u{241b}"
    );
}

#[test]
fn decoded_user_message_text_is_filtered_by_the_allowlist_again() {
    // A decoded `ESC` and a raw one the source filter kept are the same byte by
    // now, so styling survives and everything else goes.
    assert_eq!(
        sanitize_decoded(
            "\x1b[31mred\x1b[2J\x1b[8m",
            ContentClass::UserMessage,
            SanitizeMode::Strip
        ),
        "\x1b[31mred"
    );
}

#[test]
fn decoded_text_never_closes_a_span() {
    // A span closes where the renderer stops writing the content, which a
    // single text node inside it never is.
    assert_eq!(
        sanitize_decoded("\x1b[31mred", ContentClass::ToolOutput, SanitizeMode::Strip),
        "\x1b[31mred"
    );
}

#[test]
fn decoded_text_with_nothing_to_filter_is_not_copied() {
    assert!(matches!(
        sanitize_decoded("plain", ContentClass::UserMessage, SanitizeMode::Strip),
        Cow::Borrowed("plain")
    ));
    assert!(matches!(
        sanitize_decoded("a\nb\tc", ContentClass::ModelOutput, SanitizeMode::Strip),
        Cow::Borrowed("a\nb\tc")
    ));
}

#[test]
fn decoded_text_passes_through_under_off() {
    assert_eq!(
        sanitize_decoded("\x1b[2J", ContentClass::ModelOutput, SanitizeMode::Off),
        "\x1b[2J"
    );
}

#[test]
fn strip_controls_removes_every_control_character() {
    // C0 (a line feed, a tab, a carriage return), DEL, and C1. Only the `ESC`
    // of an escape sequence is a control character, so the rest of it stays.
    assert_eq!(
        strip_controls("a\nb\tc\rd\x7fe\u{9b}f\x1b[2Jg", &[]),
        "abcdef[2Jg"
    );
}

#[test]
fn strip_controls_keeps_the_characters_it_is_given() {
    assert_eq!(
        strip_controls("a\nb\tc\r\nd\x07", &['\n', '\t']),
        "a\nb\tc\nd"
    );
}

#[test]
fn strip_controls_leaves_other_text_alone() {
    assert_eq!(
        strip_controls("jp-abc: Fix the 🦀 build", &[]),
        "jp-abc: Fix the 🦀 build"
    );
}

#[test]
fn visible_sgr_rebuilds_the_sequence_from_its_parameters() {
    assert_eq!(visible_sgr("\x1b[m").as_deref(), Some("\x1b[0m"));
    assert_eq!(visible_sgr("\x1b[01;031m").as_deref(), Some("\x1b[1;31m"));
}

#[test]
fn sanitize_unclosed_leaves_closing_the_span_to_the_caller() {
    // A truncated tool result shows its first lines, and the reset belongs after
    // them rather than at the end of the part that is cut.
    assert_eq!(
        sanitize_unclosed(
            "\x1b[31mline 1\nline 2\x1b[2J",
            ContentClass::ToolOutput,
            SanitizeMode::Strip
        ),
        "\x1b[31mline 1\nline 2"
    );
}

#[test]
fn sanitize_unclosed_drops_an_unfinished_sequence() {
    assert_eq!(
        sanitize_unclosed("text\x1b[3", ContentClass::ToolOutput, SanitizeMode::Strip),
        "text"
    );
    assert_eq!(
        sanitize_unclosed(
            "text\x1b]0;tit",
            ContentClass::ToolOutput,
            SanitizeMode::Visualize
        ),
        "text\u{241b}"
    );
}

#[test]
fn sanitize_unclosed_passes_everything_through_under_off() {
    assert_eq!(
        sanitize_unclosed("\x1b[2J\r", ContentClass::ToolOutput, SanitizeMode::Off),
        "\x1b[2J\r"
    );
}

/// `chunks` through one floor under `mode`, the way the printer feeds a stream.
fn floored(mode: SanitizeMode, chunks: &[&str]) -> String {
    let mut floor = OutputFloor::new(mode);
    chunks.iter().map(|chunk| floor.filter(chunk)).collect()
}

#[test]
fn the_floor_keeps_what_jp_draws_with() {
    // Styling, a background filled to the edge of the row, and a link to a file.
    let drawn = "\x1b[1;31mred\x1b[0m\x1b[48;5;236m\x1b[K\x1b[49m\nsee \
                 \x1b]8;;file:///tmp/a\x07open\x1b]8;;\x07";

    assert_eq!(floored(SanitizeMode::Strip, &[drawn]), drawn);
}

#[test]
fn the_floor_drops_a_carriage_return() {
    // Back at the start of the row, the text after it would overwrite what
    // JP wrote there, such as the turn and role in front of a `grep` hit.
    assert_eq!(
        floored(SanitizeMode::Strip, &["1:user:safe\r1:assistant:spoofed"]),
        "1:user:safe1:assistant:spoofed"
    );
    assert_eq!(floored(SanitizeMode::Visualize, &["a\rb"]), "a\u{241b}b");
}

#[test]
fn finishing_the_floor_closes_styling_left_open() {
    // Whatever printed it, styling still open when JP exits would color the
    // shell prompt after it.
    let mut floor = OutputFloor::new(SanitizeMode::Strip);
    assert_eq!(floor.filter("\x1b[41mred"), "\x1b[41mred");
    assert_eq!(floor.finish(), "\x1b[0m");

    let mut floor = OutputFloor::new(SanitizeMode::Strip);
    assert_eq!(floor.filter("\x1b[41mred\x1b[0m"), "\x1b[41mred\x1b[0m");
    assert_eq!(floor.finish(), "");

    let mut floor = OutputFloor::new(SanitizeMode::Strip);
    assert_eq!(floor.filter("plain"), "plain");
    assert_eq!(floor.finish(), "");
}

#[test]
fn finishing_the_floor_drops_a_sequence_left_unfinished() {
    let mut floor = OutputFloor::new(SanitizeMode::Visualize);
    assert_eq!(floor.filter("a\x1b[2"), "a");
    assert_eq!(floor.finish(), "\u{241b}");
}

#[test]
fn the_floor_drops_what_takes_over_the_terminal() {
    // A screen clear, a cursor move, a whole-line erase, the alternate screen, a
    // window title, a clipboard write, a device control string, conceal, and a
    // bell.
    let hostile = concat!(
        "a\x1b[2J",
        "b\x1b[1A",
        "c\x1b[2K",
        "d\x1b[?1049h",
        "e\x1b]0;t\x07",
        "f\x1b]52;c;eA==\x07",
        "g\x1bPq\x1b\\",
        "h\x1b[8m",
        "i\x07",
        "j",
    );

    assert_eq!(floored(SanitizeMode::Strip, &[hostile]), "abcdefghij");
}

#[test]
fn the_floor_writes_an_erase_to_the_end_of_the_line_one_way() {
    assert_eq!(floored(SanitizeMode::Strip, &["a\x1b[0Kb"]), "a\x1b[Kb");
}

#[test]
fn the_floor_keeps_a_link_ended_by_st() {
    let link = "\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\";

    assert_eq!(floored(SanitizeMode::Strip, &[link]), link);
}

#[test]
fn a_link_left_open_is_closed_before_its_line_break() {
    // A line cut to fit the terminal keeps a link's opener and loses its
    // closer. Left open, every row after it links to the same target.
    assert_eq!(
        floored(SanitizeMode::Strip, &["see \x1b]8;;http://x\x07link\nnext"]),
        "see \x1b]8;;http://x\x07link\x1b]8;;\x07\nnext"
    );
}

#[test]
fn finishing_the_floor_closes_a_link_left_open() {
    // The link closes ahead of the styling reset, and both before the shell
    // prompt that follows JP.
    let mut floor = OutputFloor::new(SanitizeMode::Strip);
    assert_eq!(
        floor.filter("\x1b[31m\x1b]8;;http://x\x07red"),
        "\x1b[31m\x1b]8;;http://x\x07red"
    );
    assert_eq!(floor.finish(), "\x1b]8;;\x07\x1b[0m");
}

#[test]
fn a_closed_link_is_left_as_it_is() {
    // JP's own links close on the line they open on, with either terminator,
    // and with a `;` in the target.
    let links = concat!(
        "\x1b]8;;http://x/a;b\x07one\x1b]8;;\x07\n",
        "\x1b]8;id=1;http://y\x1b\\two\x1b]8;;\x1b\\\n",
    );

    let mut floor = OutputFloor::new(SanitizeMode::Strip);
    assert_eq!(floor.filter(links), links);
    assert_eq!(floor.finish(), "");
}

#[test]
fn a_link_target_loses_its_control_characters() {
    // A terminal that reads 8-bit controls ends the link at U+009C and takes
    // what follows as input.
    assert_eq!(
        floored(SanitizeMode::Strip, &[
            "\x1b]8;;http://x/\u{9c}y\x07z\x1b]8;;\x07"
        ]),
        "\x1b]8;;http://x/y\x07z\x1b]8;;\x07"
    );
}

#[test]
fn the_floor_marks_what_it_drops_under_visualize() {
    assert_eq!(
        floored(SanitizeMode::Visualize, &["a\x1b[2Jb\x1b]0;t\x07c"]),
        "a\u{241b}b\u{241b}c"
    );
}

#[test]
fn the_floor_lets_everything_through_under_off() {
    assert_eq!(floored(SanitizeMode::Off, &["a\x1b[2Jb"]), "a\x1b[2Jb");
}

#[test]
fn the_floor_recognizes_a_sequence_split_across_calls() {
    // `write!` hands the printer its arguments one at a time.
    assert_eq!(
        floored(SanitizeMode::Strip, &["a\x1b[", "2", "Jb\x1b[3", "1mc"]),
        "ab\x1b[31mc"
    );
}

#[test]
fn changing_the_floor_mode_drops_a_sequence_in_progress() {
    let mut floor = OutputFloor::new(SanitizeMode::Strip);

    assert_eq!(floor.filter("a\x1b["), "a");
    floor.set_mode(SanitizeMode::Off);
    assert_eq!(floor.filter("2Jb"), "2Jb");
}
