use super::*;

#[test]
fn releasing_resets_styling_shows_the_cursor_and_leaves_the_rows_the_widget_drew() {
    assert_eq!(release_sequence(false), "\x1b[0m\x1b[?25h\r\n\x1b[J");
}

#[test]
fn releasing_an_open_reply_first_undoes_the_modes_reedline_set() {
    assert_eq!(
        release_sequence(true),
        "\x1b[?2004l\x1b[<1u\x1b[0m\x1b[?25h\r\n\x1b[J"
    );
}
