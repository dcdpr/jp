use super::*;

fn spec(text: &str) -> PositionSpec {
    text.parse().unwrap()
}

#[test]
fn position_is_one_based_from_either_end() {
    assert_eq!("1".parse::<Position>(), Ok(Position::Absolute(1)));
    assert_eq!("7".parse::<Position>(), Ok(Position::Absolute(7)));
    assert_eq!("-1".parse::<Position>(), Ok(Position::FromEnd(1)));
    assert_eq!("-7".parse::<Position>(), Ok(Position::FromEnd(7)));
}

#[test]
fn position_rejects_zero_from_either_end() {
    assert_eq!("0".parse::<Position>(), Err(ParsePositionError::Zero));
    assert_eq!(
        "-0".parse::<Position>(),
        Err(ParsePositionError::ZeroFromEnd)
    );
}

#[test]
fn position_rejects_text_that_is_not_a_whole_number() {
    // `usize` accepts a leading `+`, which no position is written with.
    for text in ["", "x", "-x", "-", "--1", "1.5", "1 ", "+3", "-+3", "+0"] {
        assert_eq!(
            text.parse::<Position>(),
            Err(ParsePositionError::NotANumber(text.to_owned())),
            "accepted {text:?}"
        );
    }
}

#[test]
fn position_displays_as_written() {
    assert_eq!(Position::Absolute(4).to_string(), "4");
    assert_eq!(Position::FromEnd(4).to_string(), "-4");
}

#[test]
fn position_index_is_zero_based() {
    assert_eq!(Position::Absolute(1).index(5), Some(0));
    assert_eq!(Position::Absolute(5).index(5), Some(4));
    assert_eq!(Position::FromEnd(1).index(5), Some(4));
    assert_eq!(Position::FromEnd(5).index(5), Some(0));
    assert_eq!(Position::LAST.index(1), Some(0));
}

#[test]
fn position_index_is_none_outside_the_sequence() {
    assert_eq!(Position::Absolute(6).index(5), None);
    assert_eq!(Position::FromEnd(6).index(5), None);
    assert_eq!(Position::LAST.index(0), None);

    // `0` is not a position the parser produces, and it names no item.
    assert_eq!(Position::Absolute(0).index(5), None);
    assert_eq!(Position::FromEnd(0).index(5), None);
}

#[test]
fn spec_parses_a_single_position() {
    assert_eq!(
        "3".parse::<PositionSpec>(),
        Ok(PositionSpec::Single(Position::Absolute(3)))
    );
    assert_eq!("-1".parse::<PositionSpec>(), Ok(PositionSpec::LAST));
}

#[test]
fn spec_parses_a_range_with_either_end_open() {
    assert_eq!(
        "1..5".parse::<PositionSpec>(),
        Ok(PositionSpec::Range(
            Some(Position::Absolute(1)),
            Some(Position::Absolute(5))
        ))
    );
    assert_eq!(
        "10..".parse::<PositionSpec>(),
        Ok(PositionSpec::Range(Some(Position::Absolute(10)), None))
    );
    assert_eq!(
        "..10".parse::<PositionSpec>(),
        Ok(PositionSpec::Range(None, Some(Position::Absolute(10))))
    );
    assert_eq!("..".parse::<PositionSpec>(), Ok(PositionSpec::ALL));
}

#[test]
fn spec_range_ends_may_count_from_the_end() {
    assert_eq!(
        "-3..-2".parse::<PositionSpec>(),
        Ok(PositionSpec::Range(
            Some(Position::FromEnd(3)),
            Some(Position::FromEnd(2))
        ))
    );
    assert_eq!(
        "-3..".parse::<PositionSpec>(),
        Ok(PositionSpec::Range(Some(Position::FromEnd(3)), None))
    );
    assert_eq!(
        "..-2".parse::<PositionSpec>(),
        Ok(PositionSpec::Range(None, Some(Position::FromEnd(2))))
    );
    assert_eq!(
        "2..-1".parse::<PositionSpec>(),
        Ok(PositionSpec::Range(
            Some(Position::Absolute(2)),
            Some(Position::FromEnd(1))
        ))
    );
}

#[test]
fn spec_reports_the_end_that_failed_to_parse() {
    assert_eq!("0".parse::<PositionSpec>(), Err(ParsePositionError::Zero));
    assert_eq!(
        "0..5".parse::<PositionSpec>(),
        Err(ParsePositionError::Zero)
    );
    assert_eq!(
        "1..-0".parse::<PositionSpec>(),
        Err(ParsePositionError::ZeroFromEnd)
    );
    assert_eq!(
        "5..x".parse::<PositionSpec>(),
        Err(ParsePositionError::NotANumber("x".to_owned()))
    );
    assert_eq!(
        "".parse::<PositionSpec>(),
        Err(ParsePositionError::NotANumber(String::new()))
    );

    // The separator is `..`, with no `..=` or `...` form.
    assert_eq!(
        "1..=5".parse::<PositionSpec>(),
        Err(ParsePositionError::NotANumber("=5".to_owned()))
    );
    assert_eq!(
        "1...5".parse::<PositionSpec>(),
        Err(ParsePositionError::NotANumber(".5".to_owned()))
    );
}

#[test]
fn spec_displays_as_written() {
    for text in ["3", "-1", "1..5", "10..", "..10", "..", "-3..-2", "2..-1"] {
        assert_eq!(spec(text).to_string(), text);
    }
}

#[test]
fn resolve_returns_half_open_zero_based_indices() {
    assert_eq!(spec("3").resolve(5), Ok(2..3));
    assert_eq!(spec("-1").resolve(5), Ok(4..5));
    assert_eq!(spec("2..4").resolve(5), Ok(1..4));
    assert_eq!(spec("2..").resolve(5), Ok(1..5));
    assert_eq!(spec("..2").resolve(5), Ok(0..2));
    assert_eq!(spec("..").resolve(5), Ok(0..5));
    assert_eq!(spec("-3..-2").resolve(5), Ok(2..4));
    assert_eq!(spec("2..-1").resolve(5), Ok(1..5));

    // Equal ends are a one-item range, not a crossing.
    assert_eq!(spec("3..3").resolve(5), Ok(2..3));
}

#[test]
fn resolve_of_the_whole_of_an_empty_sequence_is_empty() {
    assert_eq!(spec("..").resolve(0), Ok(0..0));
}

#[test]
fn resolve_rejects_an_end_outside_the_sequence() {
    assert_eq!(
        spec("7").resolve(5),
        Err(ResolvePositionError::OutOfRange {
            position: Position::Absolute(7),
            count: 5
        })
    );
    assert_eq!(spec("7").resolve(7), Ok(6..7));

    assert_eq!(
        spec("2..9").resolve(5),
        Err(ResolvePositionError::OutOfRange {
            position: Position::Absolute(9),
            count: 5
        })
    );
    assert_eq!(
        spec("-6").resolve(5),
        Err(ResolvePositionError::OutOfRange {
            position: Position::FromEnd(6),
            count: 5
        })
    );

    // An open end is the edge of the sequence, but a closed end of an empty
    // sequence names an item it does not have.
    assert_eq!(
        spec("1..").resolve(0),
        Err(ResolvePositionError::OutOfRange {
            position: Position::Absolute(1),
            count: 0
        })
    );
}

#[test]
fn resolve_rejects_a_range_that_ends_before_it_starts() {
    assert_eq!(
        spec("4..2").resolve(5),
        Err(ResolvePositionError::Inverted {
            from: Position::Absolute(4),
            to: Position::Absolute(2),
            from_index: 3,
            to_index: 1,
        })
    );

    // From-end positions cross too: `-1..-3` reads as ascending.
    assert_eq!(
        spec("-1..-3").resolve(5),
        Err(ResolvePositionError::Inverted {
            from: Position::FromEnd(1),
            to: Position::FromEnd(3),
            from_index: 4,
            to_index: 2,
        })
    );
    assert_eq!(spec("-3..-1").resolve(5), Ok(2..5));
}

// An end outside the sequence is reported before the ends are compared: the
// comparison needs both to name an item.
#[test]
fn resolve_checks_the_ends_before_their_order() {
    assert_eq!(
        spec("9..2").resolve(5),
        Err(ResolvePositionError::OutOfRange {
            position: Position::Absolute(9),
            count: 5
        })
    );
}

#[test]
fn clamp_returns_half_open_zero_based_indices() {
    assert_eq!(PositionSpec::LAST.clamp(5), Some(4..5));
    assert_eq!(PositionSpec::LAST.clamp(1), Some(0..1));
    assert_eq!(PositionSpec::ALL.clamp(5), Some(0..5));
    assert_eq!(spec("-3").clamp(10), Some(7..8));
    assert_eq!(spec("-3..").clamp(10), Some(7..10));
    assert_eq!(spec("2..4").clamp(10), Some(1..4));
    assert_eq!(spec("..-2").clamp(5), Some(0..4));
}

#[test]
fn clamp_pulls_an_overshooting_end_onto_the_sequence() {
    // A start counted from before the first item starts at the first.
    assert_eq!(spec("-3..").clamp(2), Some(0..2));

    // An end past the last item ends at the last.
    assert_eq!(spec("2..4").clamp(3), Some(1..3));
}

#[test]
fn clamp_is_none_when_no_item_is_left() {
    assert_eq!(PositionSpec::LAST.clamp(0), None);
    assert_eq!(PositionSpec::ALL.clamp(0), None);

    // A start past the last item.
    assert_eq!(spec("2..4").clamp(1), None);
    assert_eq!(spec("5").clamp(3), None);

    // An end counted from before the first item.
    assert_eq!(spec("..-3").clamp(2), None);
    assert_eq!(spec("-3").clamp(2), None);

    // A range that ends before it starts.
    assert_eq!(spec("4..2").clamp(5), None);
}

// `0` is not a position the parser produces, but the variants can hold it.
#[test]
fn clamp_of_a_zero_position_stays_inside_the_sequence() {
    let zero_from_end = PositionSpec::Range(None, Some(Position::FromEnd(0)));
    assert_eq!(zero_from_end.clamp(3), Some(0..3));

    let zero = PositionSpec::Range(Some(Position::Absolute(0)), None);
    assert_eq!(zero.clamp(3), Some(0..3));
}

#[test]
fn parse_error_names_the_item() {
    assert_eq!(
        ParsePositionError::NotANumber("x".to_owned()).describe("turn"),
        "invalid turn number `x`"
    );
    assert_eq!(
        ParsePositionError::Zero.describe("turn"),
        "turn numbers are 1-based; `0` is not a valid turn"
    );
    assert_eq!(
        ParsePositionError::ZeroFromEnd.describe("turn"),
        "from-end offsets are 1-based; use `-1` for the last turn"
    );
}

#[test]
fn resolve_error_names_the_item() {
    assert_eq!(
        spec("-6").resolve(5).unwrap_err().describe("turn"),
        "turn -6 out of range (conversation has 5 turns)"
    );
    assert_eq!(
        spec("4..2").resolve(5).unwrap_err().describe("turn"),
        "range 4..2 is not valid: turn 2 comes before turn 4"
    );

    // The ends are shown as written, and as the items they resolve to.
    assert_eq!(
        spec("-1..-3")
            .resolve(5)
            .unwrap_err()
            .describe("compaction"),
        "range -1..-3 is not valid: compaction 3 comes before compaction 5"
    );
}

#[test]
fn resolve_error_counts_one_item_in_the_singular() {
    assert_eq!(
        spec("2").resolve(1).unwrap_err().describe("compaction"),
        "compaction 2 out of range (conversation has 1 compaction)"
    );
    assert_eq!(
        spec("2").resolve(0).unwrap_err().describe("compaction"),
        "compaction 2 out of range (conversation has 0 compactions)"
    );
}

// One invocation can name several conversations, so the error says which one.
#[test]
fn resolve_error_in_a_conversation_names_it() {
    assert_eq!(
        spec("2")
            .resolve(1)
            .unwrap_err()
            .describe_in("compaction", "jp-c10"),
        "compaction 2 out of range (conversation jp-c10 has 1 compaction)"
    );
    assert_eq!(
        spec("-1..-3")
            .resolve(5)
            .unwrap_err()
            .describe_in("compaction", "jp-c10"),
        "range -1..-3 is not valid in conversation jp-c10: compaction 3 comes before compaction 5"
    );
}
