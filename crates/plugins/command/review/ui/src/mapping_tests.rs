use pretty_assertions::assert_eq;

use super::*;

fn span(start: usize, end: usize) -> Span {
    Span {
        start,
        end,
        atom: false,
    }
}

/// `Some **bold** text`, rendered as `Some `, `bold`, ` text`.
const SOURCE: &str = "Some **bold** text";

fn rendered() -> [Span; 3] {
    [span(0, 5), span(7, 11), span(13, 18)]
}

#[test]
fn a_range_covers_part_of_each_span_it_touches() {
    // `me **bo`: the end of `Some `, and the start of `bold`.
    assert_eq!(covers(&rendered(), SOURCE, [2, 9]), [
        Cover::Part {
            span: 0,
            from: 2,
            to: 5,
        },
        Cover::Part {
            span: 1,
            from: 0,
            to: 2,
        },
    ]);
}

fn part(span: usize) -> Cover {
    Cover::Part {
        span,
        from: 0,
        to: 1,
    }
}

/// Two source lines of one paragraph are two spans, with the line ending
/// between them shown as a space.
#[test]
fn covers_of_consecutive_spans_set_together_are_one_run() {
    let covers = [part(3), part(4), Cover::Whole { span: 5 }];

    assert_eq!(runs(&covers, |_, _| true), [[0, 3]]);
}

/// Spans 4 and 5 are in different paragraphs.
#[test]
fn a_run_ends_where_two_spans_are_not_set_together() {
    let covers = [part(3), part(4), part(5), part(6)];

    assert_eq!(runs(&covers, |one, other| (one, other) != (4, 5)), [
        [0, 2],
        [2, 4]
    ]);
}

#[test]
fn a_run_ends_where_a_span_between_is_not_covered() {
    let covers = [part(3), part(5)];

    assert_eq!(runs(&covers, |_, _| true), [[0, 1], [1, 2]]);
    assert_eq!(runs(&[], |_, _| true), Vec::<[usize; 2]>::new());
}

/// The `**` around `bold` render as nothing, so a range over them alone covers
/// no text.
#[test]
fn a_range_over_syntax_alone_covers_nothing() {
    assert_eq!(covers(&rendered(), SOURCE, [5, 7]), []);
    assert_eq!(covers(&rendered(), SOURCE, [9, 9]), [], "an empty range");
}

/// A DOM offset counts UTF-16 code units: `é` is one, and `😀` is two.
#[test]
fn offsets_count_utf16_code_units() {
    let source = "é😀ab";
    let spans = [span(0, source.len())];

    assert_eq!(covers(&spans, source, [2, 7]), [Cover::Part {
        span: 0,
        from: 1,
        to: 4,
    }]);
}

#[test]
fn a_range_touching_an_atom_covers_it_whole() {
    // `a \| b`, the escaped pipe rendered as an atom `|`.
    let spans = [
        span(0, 2),
        Span {
            start: 2,
            end: 4,
            atom: true,
        },
        span(4, 6),
    ];

    assert_eq!(covers(&spans, "a \\| b", [3, 5]), [
        Cover::Whole { span: 1 },
        Cover::Part {
            span: 2,
            from: 0,
            to: 1,
        },
    ]);
}

#[test]
fn a_span_not_on_character_boundaries_is_left_out() {
    assert_eq!(covers(&[span(0, 1)], "é", [0, 1]), []);
}

/// A reference definition renders to nothing; a blank line shows nothing, and
/// isn't counted.
#[test]
fn lines_rendering_to_nothing_are_counted() {
    let source = "Text\n\n[x]: https://example.com\n";
    let spans = [span(0, 4)];

    assert_eq!(hidden(&spans, source, &[[0, 4], [5, 5], [6, 30]]), 1);
}

#[test]
fn a_deletion_is_marked_before_what_follows_it() {
    // Lines were removed before byte 7 and at the very end.
    assert_eq!(marks(&rendered(), &[7, 6, 40]), [
        Mark {
            span: 1,
            after: false,
        },
        Mark {
            span: 2,
            after: true,
        },
    ]);
    assert_eq!(marks(&[], &[3]), []);
}
