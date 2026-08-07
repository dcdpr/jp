use clap::Parser as _;
use jp_conversation::RangeBound;

use super::*;

/// Parse a `TurnRange` from CLI arguments.
fn parse_range(args: &[&str]) -> TurnRange {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        range: TurnRange,
    }

    let mut argv = vec!["test"];
    argv.extend_from_slice(args);
    TestCli::try_parse_from(argv).unwrap().range
}

#[test]
fn from_end_values_parse_as_values_not_flags() {
    // A leading `-` must not be mistaken for the start of another flag, whether
    // or not the value is attached with `=`.
    let expected = Some(TurnSpec::Single(TurnPos::FromEnd(2)));
    assert_eq!(parse_range(&["--turn=-2"]).turn, expected);
    assert_eq!(parse_range(&["--turn", "-2"]).turn, expected);

    assert!(matches!(
        parse_range(&["--from", "-2"]).from,
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(1)))
    ));
    assert!(matches!(
        parse_range(&["--to", "-1"]).to,
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(0)))
    ));
}

#[test]
fn parse_bound_absolute_is_one_based() {
    // `1` is the first turn (0-based `Absolute(0)` internally).
    assert!(matches!(
        parse_bound("1").unwrap(),
        CliRangeBound::Resolved(RangeBound::Absolute(0))
    ));
    assert!(matches!(
        parse_bound("5").unwrap(),
        CliRangeBound::Resolved(RangeBound::Absolute(4))
    ));
}

#[test]
fn parse_bound_from_end_is_one_based() {
    // `-1` is the last turn (0-based `FromEnd(0)` internally).
    assert!(matches!(
        parse_bound("-1").unwrap(),
        CliRangeBound::Resolved(RangeBound::FromEnd(0))
    ));
    assert!(matches!(
        parse_bound("-3").unwrap(),
        CliRangeBound::Resolved(RangeBound::FromEnd(2))
    ));
}

#[test]
fn parse_bound_rejects_zero() {
    // `0` is not a valid 1-based turn, on either end.
    assert!(parse_bound("0").is_err());
    assert!(parse_bound("-0").is_err());
}

#[test]
fn parse_bound_accepts_last_compaction() {
    assert!(matches!(
        parse_bound("last-compaction").unwrap(),
        CliRangeBound::Resolved(RangeBound::AfterLastCompaction)
    ));

    // `last` alone names nothing; it parses as a (failing) duration.
    assert!(parse_bound("last").is_err());
}

#[test]
fn parse_to_bound_rejects_last_compaction_but_accepts_indices() {
    // The most-recent-compaction marker is start-only, so `--to` rejects it.
    assert!(parse_to_bound("last-compaction").is_err());
    assert!(parse_to_bound("3").is_ok());
    assert!(parse_to_bound("-1").is_ok());
}

#[test]
fn first_and_last_are_complete_selectors() {
    // `--first N` is the first N turns: start of conversation through turn N.
    let first = TurnRange {
        first: Some(3),
        ..Default::default()
    };
    assert!(matches!(
        first.cli_from_bound(),
        Some(CliRangeBound::Resolved(RangeBound::Absolute(0)))
    ));
    assert!(matches!(
        first.cli_to_bound(),
        Some(CliRangeBound::Resolved(RangeBound::Absolute(2)))
    ));

    // `--last N` is the last N turns: N-from-the-end through the last turn.
    let last = TurnRange {
        last: Some(3),
        ..Default::default()
    };
    assert!(matches!(
        last.cli_from_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(2)))
    ));
    assert!(matches!(
        last.cli_to_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(0)))
    ));
}

#[test]
fn first_zero_is_an_empty_selection() {
    assert!(
        TurnRange {
            first: Some(0),
            ..Default::default()
        }
        .is_empty()
    );
}

#[test]
fn parse_turn_single_and_range() {
    assert_eq!(
        parse_turn("3").unwrap(),
        TurnSpec::Single(TurnPos::Absolute(3))
    );
    assert_eq!(
        parse_turn("1..5").unwrap(),
        TurnSpec::Range(Some(TurnPos::Absolute(1)), Some(TurnPos::Absolute(5)))
    );

    // Open-ended ranges.
    assert_eq!(
        parse_turn("10..").unwrap(),
        TurnSpec::Range(Some(TurnPos::Absolute(10)), None)
    );
    assert_eq!(
        parse_turn("..10").unwrap(),
        TurnSpec::Range(None, Some(TurnPos::Absolute(10)))
    );
    assert_eq!(parse_turn("..").unwrap(), TurnSpec::Range(None, None));

    // 1-based: `0` is rejected wherever a number appears.
    assert!(parse_turn("0").is_err());
    assert!(parse_turn("0..5").is_err());
    assert!(parse_turn("1..0").is_err());

    // The separator is `..`, not `..=`.
    assert!(parse_turn("1..=5").is_err());
}

#[test]
fn turn_open_ended_ranges_set_both_bounds() {
    // `--turn 10..` is turn 10 through the last turn.
    let onward = TurnRange {
        turn: Some(TurnSpec::Range(Some(TurnPos::Absolute(10)), None)),
        ..Default::default()
    };
    assert!(matches!(
        onward.cli_from_bound(),
        Some(CliRangeBound::Resolved(RangeBound::Absolute(9)))
    ));
    assert!(matches!(
        onward.cli_to_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(0)))
    ));

    // `--turn ..` is the whole conversation.
    let all = TurnRange {
        turn: Some(TurnSpec::Range(None, None)),
        ..Default::default()
    };
    assert!(matches!(
        all.cli_from_bound(),
        Some(CliRangeBound::Resolved(RangeBound::Absolute(0)))
    ));
    assert!(matches!(
        all.cli_to_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(0)))
    ));
}

#[test]
fn parse_turn_pos_is_one_based_on_both_sides() {
    assert_eq!(parse_turn_pos("1").unwrap(), TurnPos::Absolute(1));
    assert_eq!(parse_turn_pos("7").unwrap(), TurnPos::Absolute(7));
    assert_eq!(parse_turn_pos("-1").unwrap(), TurnPos::FromEnd(1));
    assert_eq!(parse_turn_pos("-7").unwrap(), TurnPos::FromEnd(7));
    assert!(parse_turn_pos("0").is_err());
    assert!(parse_turn_pos("-0").is_err());
    assert!(parse_turn_pos("x").is_err());
    assert!(parse_turn_pos("-x").is_err());
}

#[test]
fn parse_turn_accepts_from_end_positions() {
    // `-1` is the last turn, matching `--from -1` / `--to -1`.
    assert_eq!(
        parse_turn("-1").unwrap(),
        TurnSpec::Single(TurnPos::FromEnd(1))
    );
    assert_eq!(
        parse_turn("-2").unwrap(),
        TurnSpec::Single(TurnPos::FromEnd(2))
    );

    // Both ends may count from the end, and the two conventions can be mixed.
    assert_eq!(
        parse_turn("-3..-2").unwrap(),
        TurnSpec::Range(Some(TurnPos::FromEnd(3)), Some(TurnPos::FromEnd(2)))
    );
    assert_eq!(
        parse_turn("-3..").unwrap(),
        TurnSpec::Range(Some(TurnPos::FromEnd(3)), None)
    );
    assert_eq!(
        parse_turn("..-2").unwrap(),
        TurnSpec::Range(None, Some(TurnPos::FromEnd(2)))
    );
    assert_eq!(
        parse_turn("2..-1").unwrap(),
        TurnSpec::Range(Some(TurnPos::Absolute(2)), Some(TurnPos::FromEnd(1)))
    );
}

#[test]
fn turn_from_end_selects_a_single_turn() {
    // `--turn -1` is the last turn on both ends.
    let last = TurnRange {
        turn: Some(TurnSpec::Single(TurnPos::FromEnd(1))),
        ..Default::default()
    };
    assert!(matches!(
        last.cli_from_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(0)))
    ));
    assert!(matches!(
        last.cli_to_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(0)))
    ));

    // `--turn -2` is the second-to-last turn, and nothing else.
    let second_to_last = TurnRange {
        turn: Some(TurnSpec::Single(TurnPos::FromEnd(2))),
        ..Default::default()
    };
    assert!(matches!(
        second_to_last.cli_from_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(1)))
    ));
    assert!(matches!(
        second_to_last.cli_to_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(1)))
    ));
}

#[test]
fn turn_from_end_range_is_inclusive() {
    // `--turn -3..-2` is the two turns before the last, excluding it.
    let range = TurnRange {
        turn: Some(TurnSpec::Range(
            Some(TurnPos::FromEnd(3)),
            Some(TurnPos::FromEnd(2)),
        )),
        ..Default::default()
    };
    assert!(matches!(
        range.cli_from_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(2)))
    ));
    assert!(matches!(
        range.cli_to_bound(),
        Some(CliRangeBound::Resolved(RangeBound::FromEnd(1)))
    ));
}

#[test]
fn turn_out_of_range_covers_from_end_positions() {
    // A 5-turn conversation: `-5` is the first turn, `-6` is before it.
    let oob = |s: &str| {
        TurnRange {
            turn: Some(parse_turn(s).unwrap()),
            ..Default::default()
        }
        .turn_out_of_range(5)
    };

    assert_eq!(oob("-6"), Some(TurnPos::FromEnd(6)));
    assert_eq!(oob("-5"), None);
    assert_eq!(oob("-1"), None);
    assert_eq!(oob("-8..-2"), Some(TurnPos::FromEnd(8)));
    assert_eq!(oob("-3..-2"), None);
}

#[test]
fn turn_pos_displays_as_written() {
    // The out-of-range error quotes the position the user typed.
    assert_eq!(TurnPos::Absolute(4).to_string(), "4");
    assert_eq!(TurnPos::FromEnd(4).to_string(), "-4");
}
