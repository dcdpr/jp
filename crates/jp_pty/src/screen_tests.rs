use super::*;

/// A screen showing `text`, with `\n` already in tty form.
fn screen(rows: u16, columns: u16, text: &str) -> Screen {
    let mut parser = vt100::Parser::new(rows, columns, 0);
    parser.process(text.replace('\n', "\r\n").as_bytes());

    Screen::capture(parser.screen())
}

#[test]
fn rows_are_trimmed_of_their_trailing_blanks() {
    let screen = screen(3, 10, "one\ntwo");

    assert_eq!(screen.rows(), ["one", "two", ""]);
}

#[test]
fn a_row_past_the_end_reads_empty() {
    // A predicate naming a row the screen does not have never matches, which
    // surfaces as a wait timing out rather than as a panic inside the wait.
    let screen = screen(2, 10, "one");

    assert_eq!(screen.row(0), "one");
    assert_eq!(screen.row(9), "");
}

#[test]
fn tail_takes_the_bottom_rows() {
    let screen = screen(4, 10, "one\ntwo\nthree");

    assert_eq!(screen.tail(2), ["three", ""]);
    assert_eq!(screen.tail(99), ["one", "two", "three", ""]);
}

#[test]
fn used_stops_at_the_last_row_with_content() {
    let screen = screen(6, 10, "one\ntwo");

    assert_eq!(screen.used(), ["one", "two"]);
}

#[test]
fn used_is_empty_on_a_blank_screen() {
    let empty: [String; 0] = [];

    assert_eq!(screen(3, 10, "").used(), empty);
}

#[test]
fn the_cursor_follows_the_last_write() {
    let screen = screen(4, 10, "one\ntwo");

    assert_eq!(screen.cursor(), (1, 3));
}

#[test]
fn a_row_that_fills_the_width_is_not_wrapped() {
    // A wrapped row is one physical row more than the writer counted, so the
    // boundary between "fills the row" and "spills onto the next" is the fact
    // worth pinning.
    let screen = screen(4, 5, "12345");

    assert!(!screen.wrapped(0));
    assert_eq!(screen.row(0), "12345");
}

#[test]
fn a_row_past_the_width_wraps_onto_the_next() {
    let screen = screen(4, 5, "123456");

    assert!(screen.wrapped(0));
    assert_eq!(screen.row(1), "6");
}

#[test]
fn a_screen_renders_with_its_cursor_marked() {
    // This is what a failed wait prints, so it is worth reading exactly once.
    let expected = [
        "   ┌──────┐  3x6, cursor at row 1 column 3",
        " 0 │one   │",
        " 1>│two   │",
        " 2 │      │",
        "   └──────┘",
    ]
    .join("\n");

    assert_eq!(screen(3, 6, "one\ntwo").to_string(), expected);
}
