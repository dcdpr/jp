use jp_conversation::RangeBound;

use super::*;

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
fn parse_bound_accepts_last_compaction_and_alias() {
    // `last-compaction` is canonical; `last` is a deprecated alias.
    for s in ["last-compaction", "last"] {
        assert!(
            matches!(
                parse_bound(s).unwrap(),
                CliRangeBound::Resolved(RangeBound::AfterLastCompaction)
            ),
            "`{s}` should parse as the last-compaction marker"
        );
    }
}

#[test]
fn parse_to_bound_rejects_last_compaction_but_accepts_indices() {
    // The most-recent-compaction marker is start-only, so `--to` rejects it
    // (canonical name and alias).
    assert!(parse_to_bound("last-compaction").is_err());
    assert!(parse_to_bound("last").is_err());
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
    assert!(matches!(parse_turn("3").unwrap(), TurnSpec::Single(3)));
    assert!(matches!(
        parse_turn("1..5").unwrap(),
        TurnSpec::Range(Some(1), Some(5))
    ));

    // Open-ended ranges.
    assert!(matches!(
        parse_turn("10..").unwrap(),
        TurnSpec::Range(Some(10), None)
    ));
    assert!(matches!(
        parse_turn("..10").unwrap(),
        TurnSpec::Range(None, Some(10))
    ));
    assert!(matches!(
        parse_turn("..").unwrap(),
        TurnSpec::Range(None, None)
    ));

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
        turn: Some(TurnSpec::Range(Some(10), None)),
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
fn parse_one_based_rejects_zero() {
    assert_eq!(parse_one_based("1").unwrap(), 1);
    assert_eq!(parse_one_based("7").unwrap(), 7);
    assert!(parse_one_based("0").is_err());
    assert!(parse_one_based("x").is_err());
}
