use chrono::{TimeZone as _, Utc};
use jp_conversation::{ConversationEvent, ConversationStream, RangeBound, event::ChatRequest};

use super::*;

/// A stream with `count` turns, one minute apart, starting at 2020-01-01
/// 00:00:00 UTC.
fn stream(count: usize) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    let mut events = Vec::new();
    for turn in 0..count {
        let ts = Utc
            .with_ymd_and_hms(2020, 1, 1, 0, u32::try_from(turn).unwrap(), 0)
            .unwrap();
        events.push(ConversationEvent::new(
            jp_conversation::event::TurnStart,
            ts,
        ));
        events.push(ConversationEvent::new(
            ChatRequest::from(format!("Q{turn}")),
            ts,
        ));
    }
    stream.extend(events);
    stream
}

fn windows(selection: &TurnSelection, count: usize) -> Vec<(usize, usize)> {
    selection
        .resolve(&stream(count))
        .windows()
        .iter()
        .map(|w| (w.from, w.to))
        .collect()
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

    // `last` alone names nothing; it falls through to the time parsers and
    // fails there.
    assert!(parse_bound("last").is_err());
}

#[test]
fn parse_bound_accepts_relative_and_absolute_times() {
    assert!(matches!(parse_bound("5h").unwrap(), CliRangeBound::At(_)));
    assert!(matches!(
        parse_bound("2days").unwrap(),
        CliRangeBound::At(_)
    ));

    let date = parse_bound("2026-01-01").unwrap();
    let CliRangeBound::At(dt) = date else {
        panic!("a date must parse as an instant");
    };
    assert_eq!(dt, Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());

    let rfc3339 = parse_bound("2026-01-01T12:30:00Z").unwrap();
    let CliRangeBound::At(dt) = rfc3339 else {
        panic!("an RFC 3339 timestamp must parse as an instant");
    };
    assert_eq!(dt, Utc.with_ymd_and_hms(2026, 1, 1, 12, 30, 0).unwrap());
}

#[test]
fn parse_bound_reads_a_bare_integer_as_a_turn_not_a_year() {
    // A four-digit number is ambiguous to a human but never a date here: the
    // accepted date formats all require separators, so integers stay turns.
    assert!(matches!(
        parse_bound("2026").unwrap(),
        CliRangeBound::Resolved(RangeBound::Absolute(2025))
    ));
}

#[test]
fn parse_bound_rejects_unparseable_values() {
    assert!(parse_bound("nonsense").is_err());
    assert!(parse_bound("2026-13-01").is_err());
}

#[test]
fn parse_bound_rejects_a_duration_chrono_cannot_represent() {
    // `humantime` happily parses durations far larger than chrono's date range.
    // Subtracting one from `now` panics, so an oversized value must come back as
    // a parse error rather than aborting the process mid-parse.
    for s in ["10000000000000s", "100000000years"] {
        let err = parse_bound(s).unwrap_err();
        assert!(
            err.contains("too large"),
            "`{s}` should be rejected as too large, got: {err}"
        );
    }

    // The clap-level forms reject it too, on both ends.
    assert!(parse(&["--from", "10000000000000s"]).is_err());
    assert!(parse(&["--to", "10000000000000s"]).is_err());
}

#[test]
fn keep_bounds_treat_an_unrepresentable_duration_as_covering_everything() {
    // The duration arms resolve against the stream, so an oversized value can't
    // fail at parse time. It means "the protected window covers every turn",
    // which is what `Bound::Empty` already encodes in both arms.
    let huge = RuleBound::Duration(std::time::Duration::from_secs(10_000_000_000_000));
    let events = stream(5);

    assert!(matches!(keep_first_bound(&huge, &events), Bound::Empty));
    assert!(matches!(keep_last_bound(&huge, &events), Bound::Empty));

    // And the whole selection resolves to nothing rather than panicking.
    let selection = TurnSelection {
        keep_first: Some(huge),
        ..Default::default()
    };
    assert!(selection.resolve(&events).is_empty());
}

#[test]
fn parse_to_bound_rejects_last_compaction_but_accepts_other_forms() {
    // The most-recent-compaction marker is start-only, so `--to` rejects it
    // (canonical name and alias).
    assert!(parse_to_bound("last-compaction").is_err());
    assert!(parse_to_bound("last").is_err());
    assert!(parse_to_bound("3").is_ok());
    assert!(parse_to_bound("-1").is_ok());
    assert!(parse_to_bound("2026-01-01").is_ok());
}

#[test]
fn parse_keep_last_rejects_the_last_compaction_marker() {
    // `keep_last_bound` maps the marker to `Bound::Default`, so accepting it
    // would make the flag a silent no-op — and in `compact` it would suppress
    // the rule's own `keep_last` on top of that.
    assert!(parse_keep_last("last-compaction").is_err());
    // `last` alone is not the marker, so it fails in the bound parser instead.
    assert!(parse_keep_last("last").is_err());
    // An absolute turn has no config spelling, so it fails there too.
    assert!(parse_keep_last("@3").is_err());

    // Every other bound form still parses. `-3` is a position — the third turn
    // from the end — stored as the 0-based offset 2.
    assert_eq!(parse_keep_last("2").unwrap(), RuleBound::Turns(2));
    assert_eq!(parse_keep_last("-3").unwrap(), RuleBound::FromEnd(2));
    assert!(matches!(
        parse_keep_last("5h").unwrap(),
        RuleBound::Duration(_)
    ));
}

#[test]
fn clap_rejects_last_compaction_only_at_the_end_of_the_selection() {
    // Start-side bounds accept the marker; end-side bounds reject it.
    assert!(parse(&["--from", "last-compaction"]).is_ok());
    assert!(parse(&["--keep-first", "last-compaction"]).is_ok());
    assert!(parse(&["--to", "last-compaction"]).is_err());
    assert!(parse(&["--keep-last", "last-compaction"]).is_err());
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
fn turn_pos_displays_as_written() {
    // The out-of-range error quotes the position the user typed.
    assert_eq!(TurnPos::Absolute(4).to_string(), "4");
    assert_eq!(TurnPos::FromEnd(4).to_string(), "-4");
}

/// Parse a selection from CLI arguments.
fn parse(args: &[&str]) -> Result<TurnSelection, clap::Error> {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        selection: TurnSelection,
    }

    let mut argv = vec!["test"];
    argv.extend_from_slice(args);
    clap::Parser::try_parse_from(argv).map(|cli: TestCli| cli.selection)
}

#[test]
fn clap_accepts_from_end_offsets_as_separate_values() {
    // Without `allow_negative_numbers`, clap reads `-1` as an unknown short flag
    // rather than as the value of the preceding option.
    for args in [
        &["--to", "-1"][..],
        &["--to=-1"][..],
        &["--from", "-3"][..],
        &["--keep-last", "-2"][..],
    ] {
        assert!(parse(args).is_ok(), "{args:?} should parse");
    }
}

#[test]
fn clap_conflicts_the_three_ways_of_naming_a_base_range() {
    for args in [
        &["--turn", "3", "--first", "2"][..],
        &["--turn", "3", "--from", "2"][..],
        &["--first", "2", "--from", "2"][..],
        &["--last", "2", "--to", "2"][..],
    ] {
        assert!(parse(args).is_err(), "{args:?} should conflict");
    }
}

#[test]
fn clap_composes_first_with_last_and_keep_with_everything() {
    for args in [
        &["--first", "2", "--last", "2"][..],
        &["--first", "2", "--keep-first", "1"][..],
        &["--turn", "1..5", "--keep-last", "1"][..],
        &["--from", "2", "--keep-first", "1", "--keep-last", "1"][..],
    ] {
        assert!(parse(args).is_ok(), "{args:?} should compose");
    }
}

#[test]
fn clap_accepts_short_first_and_last() {
    assert!(parse(&["-f", "2", "-l", "3"]).is_ok());
    // Bare `-f` / `-l` default to one turn.
    let selection = parse(&["-f"]).unwrap();
    assert_eq!(windows(&selection, 5), vec![(0, 0)]);
}

#[test]
fn no_selector_selects_every_turn() {
    assert_eq!(windows(&TurnSelection::default(), 5), vec![(0, 4)]);
}

#[test]
fn no_selector_on_an_empty_conversation_selects_nothing() {
    assert!(TurnSelection::default().resolve(&stream(0)).is_empty());
}

#[test]
fn first_and_last_are_separate_windows() {
    let selection = TurnSelection {
        first: Some(1),
        last: Some(2),
        ..Default::default()
    };

    // 6 turns: keep turn 1 and turns 5-6, skipping 2-4.
    assert_eq!(windows(&selection, 6), vec![(0, 0), (4, 5)]);
}

#[test]
fn abutting_first_and_last_windows_merge() {
    let selection = TurnSelection {
        first: Some(2),
        last: Some(2),
        ..Default::default()
    };

    // 4 turns: the two windows meet, so the whole conversation is one window.
    assert_eq!(windows(&selection, 4), vec![(0, 3)]);
    // 3 turns: they overlap on the middle turn; still one window, no duplicate.
    assert_eq!(windows(&selection, 3), vec![(0, 2)]);
}

#[test]
fn first_alone_selects_a_leading_window() {
    assert_eq!(
        windows(
            &TurnSelection {
                first: Some(3),
                ..Default::default()
            },
            6
        ),
        vec![(0, 2)]
    );
}

#[test]
fn last_alone_selects_a_trailing_window() {
    assert_eq!(
        windows(
            &TurnSelection {
                last: Some(3),
                ..Default::default()
            },
            6
        ),
        vec![(3, 5)]
    );
}

#[test]
fn zero_count_selects_nothing() {
    for selection in [
        TurnSelection {
            first: Some(0),
            ..Default::default()
        },
        TurnSelection {
            last: Some(0),
            ..Default::default()
        },
    ] {
        assert!(selection.resolve(&stream(5)).is_empty());
    }
}

#[test]
fn zero_count_on_one_side_leaves_the_other_window() {
    // `--first 0 --last 2` is the union of an empty window and the last two
    // turns, so only the trailing window survives.
    let selection = TurnSelection {
        first: Some(0),
        last: Some(2),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 5), vec![(3, 4)]);
}

#[test]
fn turn_open_ended_ranges_cover_both_ends() {
    // `--turn 3..` is turn 3 through the last turn.
    let onward = TurnSelection {
        turn: Some(TurnSpec::Range(Some(TurnPos::Absolute(3)), None)),
        ..Default::default()
    };
    assert_eq!(windows(&onward, 5), vec![(2, 4)]);

    // `--turn ..3` is the first three turns.
    let up_to = TurnSelection {
        turn: Some(TurnSpec::Range(None, Some(TurnPos::Absolute(3)))),
        ..Default::default()
    };
    assert_eq!(windows(&up_to, 5), vec![(0, 2)]);

    // `--turn ..` is the whole conversation.
    let all = TurnSelection {
        turn: Some(TurnSpec::Range(None, None)),
        ..Default::default()
    };
    assert_eq!(windows(&all, 5), vec![(0, 4)]);
}

#[test]
fn from_and_to_are_inclusive_on_both_ends() {
    let selection = TurnSelection {
        from: Some(parse_bound("2").unwrap()),
        to: Some(parse_bound("4").unwrap()),
        ..Default::default()
    };

    // `--from 2 --to 4` is three turns, matching `--turn 2..4`.
    assert_eq!(windows(&selection, 6), vec![(1, 3)]);
}

#[test]
fn to_minus_one_is_the_last_turn() {
    let selection = TurnSelection {
        from: Some(parse_bound("3").unwrap()),
        to: Some(parse_bound("-1").unwrap()),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 5), vec![(2, 4)]);
}

#[test]
fn an_absolute_date_bound_names_a_whole_turn() {
    // Turns run 00:00, 00:01, ... 00:04. A `--to` cutoff mid-conversation ends
    // the selection at the turn active then, never inside it.
    let selection = TurnSelection {
        to: Some(parse_to_bound("2020-01-01T00:02:30Z").unwrap()),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 5), vec![(0, 2)]);
}

#[test]
fn a_from_time_bound_starts_at_the_next_turn_to_begin() {
    // The turn active at 00:02:30 started before the cutoff, so `--from` starts
    // at turn 4 (index 3) — the first turn to begin after it.
    let selection = TurnSelection {
        from: Some(parse_bound("2020-01-01T00:02:30Z").unwrap()),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 5), vec![(3, 4)]);
}

#[test]
fn a_to_cutoff_before_the_conversation_selects_nothing() {
    let selection = TurnSelection {
        to: Some(parse_to_bound("2019-01-01").unwrap()),
        ..Default::default()
    };
    assert!(selection.resolve(&stream(5)).is_empty());
}

#[test]
fn keep_first_protects_the_leading_turns() {
    let selection = TurnSelection {
        keep_first: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 5), vec![(2, 4)]);
}

#[test]
fn keep_last_protects_the_trailing_turns() {
    let selection = TurnSelection {
        keep_last: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 5), vec![(0, 2)]);
}

#[test]
fn keep_flags_narrow_a_first_window() {
    // `--keep-first 1 --first 4` selects turns 2 through 4.
    let selection = TurnSelection {
        first: Some(4),
        keep_first: Some(RuleBound::Turns(1)),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 8), vec![(1, 3)]);
}

#[test]
fn keep_flags_narrow_a_last_window() {
    // `--keep-last 2 --last 4` selects the two turns before the final two.
    let selection = TurnSelection {
        last: Some(4),
        keep_last: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 8), vec![(4, 5)]);
}

#[test]
fn keep_first_clamps_rather_than_shifts_an_explicit_range() {
    // Turn 1 is already outside `--from 3`, so nothing needs protecting and the
    // window is unchanged. A shifting trim would wrongly start at turn 4.
    let selection = TurnSelection {
        from: Some(parse_bound("3").unwrap()),
        keep_first: Some(RuleBound::Turns(1)),
        ..Default::default()
    };
    assert_eq!(windows(&selection, 6), vec![(2, 5)]);
}

#[test]
fn keep_flags_apply_to_every_window() {
    let selection = TurnSelection {
        first: Some(3),
        last: Some(3),
        keep_first: Some(RuleBound::Turns(1)),
        keep_last: Some(RuleBound::Turns(1)),
        ..Default::default()
    };

    // 10 turns: leading window 1-3 loses turn 1, trailing window 8-10 loses
    // turn 10.
    assert_eq!(windows(&selection, 10), vec![(1, 2), (7, 8)]);
}

#[test]
fn keep_flags_covering_the_selection_select_nothing() {
    let selection = TurnSelection {
        keep_first: Some(RuleBound::Turns(3)),
        keep_last: Some(RuleBound::Turns(3)),
        ..Default::default()
    };
    assert!(selection.resolve(&stream(4)).is_empty());
}

#[test]
fn validate_rejects_a_keep_that_swallows_the_selection() {
    let selection = TurnSelection {
        first: Some(1),
        keep_first: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_err());

    let selection = TurnSelection {
        last: Some(1),
        keep_last: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_err());

    // Equal counts are allowed: the selection is empty but not contradictory.
    let selection = TurnSelection {
        first: Some(2),
        keep_first: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_ok());
}

#[test]
fn validate_allows_a_swallowed_window_when_the_other_survives() {
    // `--first`/`--last` are a union, so a keep flag covering one window is not
    // a contradiction while the other window still names turns.
    let selection = TurnSelection {
        first: Some(1),
        last: Some(2),
        keep_first: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_ok());
    // Only the trailing window survives.
    assert_eq!(windows(&selection, 8), vec![(6, 7)]);

    // The mirror image: `--keep-last` swallows the trailing window.
    let selection = TurnSelection {
        first: Some(2),
        last: Some(1),
        keep_last: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_ok());
    assert_eq!(windows(&selection, 8), vec![(0, 1)]);
}

#[test]
fn validate_rejects_keeps_that_swallow_both_windows() {
    // Neither window survives, so the pair is contradictory after all.
    let selection = TurnSelection {
        first: Some(1),
        last: Some(1),
        keep_first: Some(RuleBound::Turns(2)),
        keep_last: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_err());

    // An explicit zero on the other side is not a surviving window either.
    let selection = TurnSelection {
        first: Some(1),
        last: Some(0),
        keep_first: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_err());
}

#[test]
fn validate_rejects_a_swallowed_window_beside_an_exactly_covered_one() {
    // `--keep-last 1 --last 1` names no turn on any stream, so it cannot be the
    // survivor that excuses `--keep-first 2 --first 1`. Recognising only
    // `keep > count` here would let this through while the `--keep-first 2
    // --first 1` pair errors on its own.
    let selection = TurnSelection {
        first: Some(1),
        last: Some(1),
        keep_first: Some(RuleBound::Turns(2)),
        keep_last: Some(RuleBound::Turns(1)),
        ..Default::default()
    };
    assert!(selection.validate().is_err());
    assert!(selection.resolve(&stream(8)).is_empty());

    // The mirror image.
    let selection = TurnSelection {
        first: Some(1),
        last: Some(1),
        keep_first: Some(RuleBound::Turns(1)),
        keep_last: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_err());
    assert!(selection.resolve(&stream(8)).is_empty());
}

#[test]
fn validate_allows_equal_keep_and_count_on_both_windows() {
    // Equality alone stays allowed — empty but not contradictory — and that
    // policy doesn't change just because both windows hit it.
    let selection = TurnSelection {
        first: Some(2),
        last: Some(2),
        keep_first: Some(RuleBound::Turns(2)),
        keep_last: Some(RuleBound::Turns(2)),
        ..Default::default()
    };
    assert!(selection.validate().is_ok());
}

#[test]
fn check_turn_range_rejects_endpoints_past_the_conversation() {
    let selection = TurnSelection {
        turn: Some(TurnSpec::Single(TurnPos::Absolute(7))),
        ..Default::default()
    };
    assert!(selection.check_turn_range(5).is_err());
    assert!(selection.check_turn_range(7).is_ok());

    let selection = TurnSelection {
        turn: Some(TurnSpec::Range(
            Some(TurnPos::Absolute(2)),
            Some(TurnPos::Absolute(9)),
        )),
        ..Default::default()
    };
    assert!(selection.check_turn_range(5).is_err());

    // A from-end endpoint is checked against the same count, and the error
    // quotes it as written.
    let selection = TurnSelection {
        turn: Some(TurnSpec::Single(TurnPos::FromEnd(6))),
        ..Default::default()
    };
    assert_eq!(
        selection.check_turn_range(5).unwrap_err(),
        "turn -6 out of range (conversation has 5 turns)"
    );
    assert!(selection.check_turn_range(6).is_ok());

    // Counts clamp instead of erroring, so `--first` is never out of range.
    let selection = TurnSelection {
        first: Some(99),
        ..Default::default()
    };
    assert!(selection.check_turn_range(5).is_ok());
}

#[test]
fn is_set_reports_whether_the_user_named_a_subset() {
    assert!(!TurnSelection::default().is_set());
    assert!(
        TurnSelection {
            keep_last: Some(RuleBound::Turns(1)),
            ..Default::default()
        }
        .is_set()
    );
}

#[test]
fn turn_set_contains_only_selected_turns() {
    let selection = TurnSelection {
        first: Some(1),
        last: Some(1),
        ..Default::default()
    };
    let set = selection.resolve(&stream(4));

    assert!(set.contains(0));
    assert!(!set.contains(1));
    assert!(!set.contains(2));
    assert!(set.contains(3));
}
