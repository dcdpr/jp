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
fn a_deselection_drops_the_range_when_the_actions_were_never_pressed() {
    assert!(!kept(None, 10_000.0));
}

#[test]
fn a_deselection_while_the_actions_are_pressed_keeps_the_range() {
    assert!(kept(Some(10_000.0), 10_150.0));
}

#[test]
fn a_deselection_long_after_the_last_press_drops_the_range() {
    assert!(!kept(Some(10_000.0), 10_000.0 + PRESS_WINDOW));
}

#[test]
fn a_press_after_the_deselection_does_not_keep_it() {
    assert!(!kept(Some(10_200.0), 10_000.0));
}

#[test]
fn ends_inside_text_map_character_for_character() {
    // `me ` of the first span through `bo` of the second.
    let bytes = anchor(
        &rendered(),
        SOURCE,
        End::In { span: 0, offset: 2 },
        End::In { span: 1, offset: 2 },
    );

    assert_eq!(bytes, Ok([2, 9]));
    assert_eq!(&SOURCE[2..9], "me **bo", "the markup between is covered");
}

/// A selection that runs past the text, as one made by tapping a paragraph
/// does, covers the spans it holds whole.
#[test]
fn ends_beyond_the_text_cover_the_spans_whole() {
    let bytes = anchor(&rendered(), SOURCE, End::Beyond { span: 1 }, End::Beyond {
        span: 2,
    });

    assert_eq!(bytes, Ok([7, 18]));
}

/// A DOM offset counts UTF-16 code units: `é` is one and `😀` two; an offset
/// inside `😀` rounds down to its start.
#[test]
fn offsets_count_utf16_code_units() {
    let source = "é😀ab";
    let spans = [span(0, source.len())];

    let whole = anchor(&spans, source, End::In { span: 0, offset: 1 }, End::In {
        span: 0,
        offset: 4,
    });
    let split = anchor(&spans, source, End::In { span: 0, offset: 0 }, End::In {
        span: 0,
        offset: 2,
    });

    assert_eq!(whole, Ok([2, 7]), "`😀a`");
    assert_eq!(split, Ok([0, 2]), "`é`, the half of `😀` dropped");
}

#[test]
fn an_atom_is_covered_whole() {
    // `a \| b`, the escaped pipe rendered as an atom `|`.
    let source = "a \\| b";
    let spans = [
        span(0, 2),
        Span {
            start: 2,
            end: 4,
            atom: true,
        },
        span(4, 6),
    ];

    let bytes = anchor(&spans, source, End::In { span: 1, offset: 1 }, End::In {
        span: 1,
        offset: 1,
    });

    assert_eq!(bytes, Ok([2, 4]));
}

#[test]
fn an_end_in_unmapped_text_is_refused() {
    let refused = anchor(
        &rendered(),
        SOURCE,
        End::In { span: 0, offset: 1 },
        End::Unmapped,
    );

    assert_eq!(refused, Err(Refusal::Unmapped));
}

/// From the end of `Some ` to the start of `bold` holds only the `**` that
/// renders as nothing.
#[test]
fn a_selection_holding_no_rendered_text_is_empty() {
    let syntax = anchor(
        &rendered(),
        SOURCE,
        End::In { span: 0, offset: 5 },
        End::In { span: 1, offset: 0 },
    );
    let collapsed = anchor(
        &rendered(),
        SOURCE,
        End::In { span: 1, offset: 2 },
        End::In { span: 1, offset: 2 },
    );

    assert_eq!(syntax, Err(Refusal::Empty));
    assert_eq!(collapsed, Err(Refusal::Empty));
}
