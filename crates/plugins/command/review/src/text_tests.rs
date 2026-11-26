use pretty_assertions::assert_eq;

use super::*;

fn range(text: &str) -> Range {
    text.parse().unwrap()
}

#[test]
fn ranges_read_and_write_in_the_form_at_takes() {
    assert_eq!(range("42"), Range::Lines {
        first: 42,
        last: 42
    });
    assert_eq!(range("42-45"), Range::Lines {
        first: 42,
        last: 45
    });
    assert_eq!(range("42:5-43:18"), Range::Span {
        start: LineColumn {
            line: 42,
            column: 5
        },
        end: LineColumn {
            line: 43,
            column: 18
        },
    });

    for text in ["42", "42-45", "42:5-43:18", "7:3-7:3"] {
        assert_eq!(range(text).to_string(), text);
    }
    assert_eq!(range("42-42").to_string(), "42");
}

#[test]
fn malformed_ranges_are_refused() {
    for text in [
        "",
        "0",
        "42-",
        "-3",
        "a",
        "+4",
        " 4",
        "42:5",
        "42:5-43",
        "42-43:18",
        "42:0-43:1",
    ] {
        let error = text.parse::<Range>().unwrap_err();

        assert!(
            error.starts_with(&format!("`{text}` is not a range;")),
            "{error}"
        );
    }
}

#[test]
fn a_range_running_backwards_is_refused() {
    assert_eq!(
        "45-42".parse::<Range>().unwrap_err(),
        "The range `45-42` ends before it starts."
    );
    assert_eq!(
        "43:1-42:9".parse::<Range>().unwrap_err(),
        "The range `43:1-42:9` ends before it starts."
    );
}

#[test]
fn a_range_is_stored_as_it_is_written() {
    assert_eq!(
        serde_json::to_string(&range("42:5-43:18")).unwrap(),
        "\"42:5-43:18\""
    );
    assert_eq!(
        serde_json::from_str::<Range>("\"60-64\"").unwrap(),
        range("60-64")
    );
    assert!(serde_json::from_str::<Range>("\"64-60\"").is_err());
}

#[test]
fn lines_keep_their_line_endings() {
    assert_eq!(Text::new("a\nb\r\nc\rd").lines(), [
        "a\n", "b\r\n", "c\r", "d"
    ]);
    assert_eq!(Text::new("a\n").lines(), ["a\n", ""]);
    assert_eq!(Text::new("").lines(), [""]);
}

#[test]
fn an_excerpt_is_the_text_a_range_covers() {
    let text = Text::new("one\ntwo\nthree\n");

    assert_eq!(text.excerpt(range("2")), Some("two"));
    assert_eq!(text.excerpt(range("1-2")), Some("one\ntwo"));
    assert_eq!(text.excerpt(range("1:2-2:3")), Some("ne\ntw"));
    assert_eq!(text.excerpt(range("1:4-2:1")), Some("\n"));
    assert_eq!(text.excerpt(range("2:1-2:1")), Some(""));
    assert_eq!(text.excerpt(range("4")), Some(""));
}

#[test]
fn columns_count_code_points() {
    let text = Text::new("aé世\n");

    assert_eq!(text.excerpt(range("1:2-1:3")), Some("é"));
    assert_eq!(text.excerpt(range("1:3-1:4")), Some("世"));
    assert_eq!(text.bytes(range("1:2-1:4")), Some(1..6));
}

#[test]
fn a_range_outside_the_text_has_no_excerpt() {
    let text = Text::new("one\ntwo");

    assert_eq!(text.excerpt(range("3")), None);
    assert_eq!(text.excerpt(range("1-3")), None);
    assert_eq!(text.excerpt(range("1:6-1:7")), None);
    assert_eq!(text.excerpt(range("2:1-2:5")), None);
}

fn project(old: &str, new: &str, range_text: &str) -> Option<String> {
    LineMap::new(&Text::new(old), &Text::new(new))
        .project(range(range_text))
        .map(|projected| projected.to_string())
}

#[test]
fn unchanged_lines_carry_a_range_to_where_they_moved() {
    let old = "a\nb\nc\n";
    let new = "x\na\nb\nc\n";

    assert_eq!(project(old, new, "2-3").as_deref(), Some("3-4"));
    assert_eq!(project(old, new, "2:1-3:2").as_deref(), Some("3:1-4:2"));
    assert_eq!(project(old, old, "1:1-2:1").as_deref(), Some("1:1-2:1"));
}

#[test]
fn a_changed_line_leaves_the_ranges_covering_it_outdated() {
    let old = "a\nb\nc\n";
    let new = "a\nB\nc\n";

    assert_eq!(project(old, new, "2"), None);
    assert_eq!(project(old, new, "1-3"), None);
    assert_eq!(project(old, new, "1:1-2:1").as_deref(), Some("1:1-2:1"));
    assert_eq!(project(old, new, "3").as_deref(), Some("3"));
}

/// The text between two unchanged lines changed, so the range no longer covers
/// the same text.
#[test]
fn a_line_inserted_inside_a_range_leaves_it_outdated() {
    let old = "a\nb\nc\n";
    let new = "a\nb\nx\nc\n";

    assert_eq!(project(old, new, "2-3"), None);
    assert_eq!(project(old, new, "2").as_deref(), Some("2"));
}

/// A span ending at the start of a line covers nothing of that line, so an edit
/// there leaves it in place.
#[test]
fn a_span_ending_at_a_line_start_ignores_that_line() {
    let old = "a\nb\nc\n";
    let new = "x\na\nb\nC\n";

    assert_eq!(project(old, new, "2:1-3:1").as_deref(), Some("3:1-4:1"));
    assert_eq!(project(old, new, "2:1-3:2"), None);
}

/// An editor or `core.autocrlf` rewriting the line endings leaves every comment
/// on its text.
#[test]
fn a_changed_line_ending_leaves_the_line_in_place() {
    let old = "a\r\nbc\r\n";
    let new = "a\nbc\n";

    assert_eq!(project(old, new, "1").as_deref(), Some("1"));
    assert_eq!(project(old, new, "2:2-2:3").as_deref(), Some("2:2-2:3"));
    assert_eq!(project("a\nb", "a\nb\n", "2").as_deref(), Some("2"));
}

fn through(old: &str, new: &str, count: usize) -> usize {
    LineMap::new(&Text::new(old), &Text::new(new)).through(count)
}

#[test]
fn the_lines_read_move_with_the_lines_added_and_removed_above_them() {
    assert_eq!(through("a\nb\nc\n", "a\nb\nc\n", 2), 2);
    assert_eq!(through("a\nb\nc\n", "x\na\nb\nc\n", 2), 3);
    assert_eq!(through("a\nb\nc\n", "b\nc\n", 2), 1);
}

/// The reader read the old line 2, and its replacement sits where it did.
#[test]
fn a_line_read_that_changed_keeps_its_place() {
    assert_eq!(through("a\nb\nc\n", "a\nB\nc\n", 2), 2);
    // Lines 2 and 3 became one line, which is as far as either reaches.
    assert_eq!(through("a\nb\nc\nd\n", "a\nX\nd\n", 2), 2);
    assert_eq!(through("a\nb\nc\nd\n", "a\nX\nd\n", 3), 2);
}

/// A line added right after the last line read was not read.
#[test]
fn lines_added_after_the_last_line_read_are_not_counted() {
    assert_eq!(through("a\nb\n", "a\nx\nb\n", 1), 1);
}

#[test]
fn nothing_is_left_of_lines_read_that_were_all_removed() {
    assert_eq!(through("a\nb\n", "b\n", 1), 0);
    assert_eq!(through("a\nb\n", "a\nb\n", 0), 0);
}

#[test]
fn the_last_line_of_a_span_ending_at_a_line_start_is_the_line_before() {
    assert_eq!(range("2-4").last_line(), 4);
    assert_eq!(range("2:1-4:3").last_line(), 4);
    assert_eq!(range("2:1-4:1").last_line(), 3);
    assert_eq!(range("2:1-2:1").last_line(), 2);
}

#[test]
fn the_contents_of_each_line_leave_its_line_ending_out() {
    assert_eq!(Text::new("a\nb\r\nc\rd").contents(), ["a", "b", "c", "d"]);
    assert_eq!(Text::new("a\n").contents(), ["a", ""]);
    assert_eq!(Text::new("").contents(), [""]);
}

/// The empty line after a final line ending carries like any other, as long as
/// the text still ends in one.
#[test]
fn the_line_after_a_final_line_ending_carries() {
    assert_eq!(
        project("a\nb\n", "x\na\nb\n", "3:1-3:1").as_deref(),
        Some("4:1-4:1")
    );
    assert_eq!(project("a\nb\n", "a\nb\nc", "3:1-3:1"), None);
}

#[test]
fn a_carried_range_covers_the_text_it_covered() {
    let old = "# Title\n\nFirst paragraph.\nSecond line.\n\n- item\n";
    let new = "# Title\n\nIntro.\n\nFirst paragraph.\nSecond line.\n\n- item\n- more\n";
    let (old_text, new_text) = (Text::new(old), Text::new(new));
    let map = LineMap::new(&old_text, &new_text);

    for text in ["1", "3", "3-4", "3:7-4:7", "4:1-5:1", "6:3-6:7"] {
        let projected = map.project(range(text)).unwrap();

        assert_eq!(
            new_text.excerpt(projected),
            old_text.excerpt(range(text)),
            "{text} -> {projected}"
        );
    }
}

/// A browser addresses text by byte offsets, which a comment keeps as lines and
/// columns, so the two have to round-trip.
#[test]
fn byte_offsets_are_read_as_a_span_of_lines_and_columns() {
    let text = Text::new("café\nnaïve\n");

    let span = text.span_of(3..10).unwrap();

    assert_eq!(span, range("1:4-2:4"));
    assert_eq!(text.excerpt(span), Some("é\nnaï"));
    assert_eq!(text.bytes(span), Some(3..10));
    assert_eq!(text.span_of(13..13), Some(range("3:1-3:1")));
}

#[test]
fn byte_offsets_outside_the_text_inside_a_character_or_backwards_are_no_span() {
    let text = Text::new("café\n");

    assert_eq!(text.span_of(0..7), None);
    assert_eq!(text.span_of(0..4), None, "inside the é");
    assert_eq!(text.span_of(ops::Range { start: 3, end: 1 }), None);
}

#[test]
fn the_lines_shown_leave_out_the_empty_line_after_a_final_line_ending() {
    assert_eq!(Text::new("a\nb\n").shown(), ["a\n", "b\n"]);
    assert_eq!(Text::new("a\nb").shown(), ["a\n", "b"]);
    assert_eq!(Text::new("").shown(), Vec::<&str>::new());
}

#[test]
fn a_quote_narrows_lines_to_its_columns() {
    let text = Text::new("# Title\n\nThe server binds to every interface.\nBy default.\n");

    let narrowed = text.narrow(range("3-4"), "every interface.\nBy").unwrap();

    assert_eq!(narrowed, range("3:21-4:3"));
    assert_eq!(text.excerpt(narrowed), Some("every interface.\nBy"));
    assert_eq!(text.narrow(range("1"), "Title").unwrap(), range("1:3-1:8"));
}

#[test]
fn a_quote_counts_columns_in_code_points() {
    let text = Text::new("café crème\n");

    assert_eq!(text.narrow(range("1"), "crème").unwrap(), range("1:6-1:11"));
}

#[test]
fn a_quote_has_to_occur_in_the_lines_exactly_once() {
    let text = Text::new("aaa\nab ab\nxyz\n");

    assert_eq!(
        text.narrow(range("1"), "aa").unwrap_err(),
        "`quote` occurs 2 times in lines 1; quote more of the line to name one of them."
    );
    assert_eq!(
        text.narrow(range("2"), "ab").unwrap_err(),
        "`quote` occurs 2 times in lines 2; quote more of the line to name one of them."
    );
    assert_eq!(
        text.narrow(range("1-2"), "xyz").unwrap_err(),
        "`quote` doesn't occur in lines 1-2."
    );
    assert_eq!(
        text.narrow(range("3"), "").unwrap_err(),
        "`quote` is empty."
    );
    assert_eq!(
        text.narrow(range("5"), "x").unwrap_err(),
        "Lines 5 lie outside the file."
    );
}

#[test]
fn hunks_show_the_changed_lines_with_context() {
    assert_eq!(hunks("a\nb\nc\n", "a\nb\nc\n"), "");
    assert_eq!(
        hunks("a\nb\nc\n", "a\nB\nc\n"),
        "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
    );
    assert_eq!(hunks("", "a\n"), "@@ -0,0 +1 @@\n+a\n");
}

#[test]
fn changes_place_added_and_modified_lines_in_the_new_text() {
    // Line 2 changed, and line 4 is new: `B` at 2..3 and `d` at 6..7.
    assert_eq!(changes("a\nb\nc\n", "a\nB\nc\nd\n"), Changes {
        added: vec![[2, 3], [6, 7]],
        removed: vec![],
    });
    assert_eq!(changes("a\nb\n", "a\nb\n"), Changes::default());
}

/// A removed run is marked where the line after it starts, and at the end of
/// the text when nothing follows it.
#[test]
fn changes_mark_where_removed_lines_were() {
    assert_eq!(changes("a\nb\nc\n", "a\nc\n"), Changes {
        added: vec![],
        removed: vec![2],
    });
    assert_eq!(changes("a\nb\nc\n", "a\n"), Changes {
        added: vec![],
        removed: vec![2],
    });
    assert_eq!(changes("a\nb", "a\n"), Changes {
        added: vec![],
        removed: vec![2],
    });
}

/// Lines put in place of removed ones are modified, and the removal leaves no
/// mark of its own.
#[test]
fn changes_count_a_replaced_run_as_modified() {
    assert_eq!(changes("a\nb\nc\nd\n", "a\nX\nd\n"), Changes {
        added: vec![[2, 3]],
        removed: vec![],
    });
}

/// The unified diff shows the line as changed, so the highlight agrees.
#[test]
fn changes_take_a_changed_line_ending_as_a_modified_line() {
    assert_eq!(changes("a\nb", "a\nb\n"), Changes {
        added: vec![[2, 3]],
        removed: vec![],
    });
    assert_eq!(changes("a\r\nb\n", "a\nb\n"), Changes {
        added: vec![[0, 1]],
        removed: vec![],
    });
}

#[test]
fn changes_in_a_new_file_add_every_line() {
    assert_eq!(changes("", "a\n\nb"), Changes {
        added: vec![[0, 1], [2, 2], [3, 4]],
        removed: vec![],
    });
}

#[test]
fn hunks_mark_a_last_line_without_a_line_ending() {
    assert_eq!(
        hunks("a\n", "a"),
        "@@ -1 +1 @@\n-a\n+a\n\\ No newline at end of file\n"
    );
}
