use pretty_assertions::assert_eq;

use super::*;

#[test]
fn line_column_counts_code_points() {
    let lines = Lines::new("aé\nb");

    assert_eq!(
        lines.line_column(0),
        Some(LineColumn { line: 1, column: 1 })
    );
    assert_eq!(
        lines.line_column(1),
        Some(LineColumn { line: 1, column: 2 })
    );
    assert_eq!(
        lines.line_column(3),
        Some(LineColumn { line: 1, column: 3 })
    );
    assert_eq!(
        lines.line_column(4),
        Some(LineColumn { line: 2, column: 1 })
    );
    assert_eq!(
        lines.line_column(5),
        Some(LineColumn { line: 2, column: 2 })
    );
}

#[test]
fn line_column_rejects_offsets_inside_a_character_or_past_the_end() {
    let lines = Lines::new("aé");

    assert_eq!(lines.line_column(2), None);
    assert_eq!(lines.line_column(4), None);
}

#[test]
fn offset_of_each_column_counts_code_points() {
    let lines = Lines::new("aé\nb");

    assert_eq!(lines.offset(LineColumn { line: 1, column: 1 }), Some(0));
    assert_eq!(lines.offset(LineColumn { line: 1, column: 2 }), Some(1));
    assert_eq!(lines.offset(LineColumn { line: 1, column: 3 }), Some(3));
    assert_eq!(lines.offset(LineColumn { line: 2, column: 1 }), Some(4));
    assert_eq!(lines.offset(LineColumn { line: 2, column: 2 }), Some(5));
}

#[test]
fn offset_rejects_positions_outside_the_text() {
    let lines = Lines::new("ab\nc");

    assert_eq!(lines.offset(LineColumn { line: 0, column: 1 }), None);
    assert_eq!(lines.offset(LineColumn { line: 1, column: 0 }), None);
    // Column 3 is the line ending; column 4 would be the next line's start.
    assert_eq!(lines.offset(LineColumn { line: 1, column: 4 }), None);
    assert_eq!(lines.offset(LineColumn { line: 2, column: 3 }), None);
    assert_eq!(lines.offset(LineColumn { line: 3, column: 1 }), None);
}

#[test]
fn crlf_and_a_lone_cr_each_end_one_line() {
    let lines = Lines::new("a\r\nb\rc");

    assert_eq!(
        lines.line_column(2),
        Some(LineColumn { line: 1, column: 3 })
    );
    assert_eq!(
        lines.line_column(3),
        Some(LineColumn { line: 2, column: 1 })
    );
    assert_eq!(
        lines.line_column(5),
        Some(LineColumn { line: 3, column: 1 })
    );
}

#[test]
fn trailing_line_ending_starts_an_empty_last_line() {
    let lines = Lines::new("a\n");

    assert_eq!(
        lines.line_column(2),
        Some(LineColumn { line: 2, column: 1 })
    );
    assert_eq!(lines.offset(LineColumn { line: 2, column: 1 }), Some(2));
}

#[test]
fn line_range_excludes_the_line_ending() {
    let lines = Lines::new("ab\r\nc\n");

    assert_eq!(lines.line_range(0), None);
    assert_eq!(lines.line_range(1), Some(0..2));
    assert_eq!(lines.line_range(2), Some(4..5));
    assert_eq!(lines.line_range(3), Some(6..6));
    assert_eq!(lines.line_range(4), None);
}

#[test]
fn offset_inverts_line_column_at_every_character_boundary() {
    let text = "aé\r\nb\rc\n世\n";
    let lines = Lines::new(text);

    for offset in (0..=text.len()).filter(|&offset| text.is_char_boundary(offset)) {
        let position = lines.line_column(offset).unwrap();
        assert_eq!(lines.offset(position), Some(offset), "{position:?}");
    }
}
