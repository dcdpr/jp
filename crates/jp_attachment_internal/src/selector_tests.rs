use jp_conversation::position::Position;

use super::*;

#[test]
fn selector_default_is_last_assistant() {
    let sel: Selector = "".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, PositionSpec::LAST);
}

#[test]
fn selector_bare_content_defaults_range_to_last_turn() {
    let sel: Selector = "a".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, PositionSpec::LAST);
}

#[test]
fn selector_parses_combined_content() {
    let sel: Selector = "u,a,r,t:..".parse().unwrap();
    assert_eq!(sel.content, Content::all());
    assert_eq!(sel.range, PositionSpec::ALL);
}

#[test]
fn selector_star_is_all_content() {
    let sel: Selector = "*:..".parse().unwrap();
    assert_eq!(sel.content, Content::all());
}

#[test]
fn selector_parses_negative_shorthand() {
    // `-3` names the third turn from the end, not the last three turns.
    let sel: Selector = "a:-3".parse().unwrap();
    assert_eq!(sel.range, PositionSpec::Single(Position::FromEnd(3)));
    assert_eq!(sel.range.clamp(10), Some(7..8));
}

#[test]
fn selector_parses_open_ended_negative_range() {
    // The last three turns need the explicit open range.
    let sel: Selector = "a:-3..".parse().unwrap();
    assert_eq!(
        sel.range,
        PositionSpec::Range(Some(Position::FromEnd(3)), None)
    );
    assert_eq!(sel.range.clamp(10), Some(7..10));
}

#[test]
fn selector_parses_positive_shorthand() {
    let sel: Selector = "a:5".parse().unwrap();
    assert_eq!(sel.range, PositionSpec::Single(Position::Absolute(5)));
}

#[test]
fn selector_parses_explicit_range() {
    let sel: Selector = "a:2..4".parse().unwrap();
    assert_eq!(
        sel.range,
        PositionSpec::Range(Some(Position::Absolute(2)), Some(Position::Absolute(4)))
    );
}

#[test]
fn selector_bare_negative_index_is_range_only() {
    // `-1` (no colon) means "default content, last turn" — same as `a:-1`.
    let sel: Selector = "-1".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, PositionSpec::LAST);
}

#[test]
fn selector_bare_positive_index_is_range_only() {
    let sel: Selector = "5".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, PositionSpec::Single(Position::Absolute(5)));
}

#[test]
fn selector_bare_full_range_is_range_only() {
    let sel: Selector = "..".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, PositionSpec::ALL);
}

#[test]
fn selector_bare_explicit_range_is_range_only() {
    let sel: Selector = "5..-3".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(
        sel.range,
        PositionSpec::Range(Some(Position::Absolute(5)), Some(Position::FromEnd(3)))
    );
}

#[test]
fn selector_colon_prefix_uses_default_content() {
    // `:-1` is the long-form equivalent of `-1`.
    let sel: Selector = ":-1".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, PositionSpec::LAST);
}

#[test]
fn selector_parses_open_ended_ranges() {
    let sel: Selector = "a:3..".parse().unwrap();
    assert_eq!(
        sel.range,
        PositionSpec::Range(Some(Position::Absolute(3)), None)
    );

    let sel: Selector = "a:..5".parse().unwrap();
    assert_eq!(
        sel.range,
        PositionSpec::Range(None, Some(Position::Absolute(5)))
    );
}

#[test]
fn selector_rejects_unknown_content_flag() {
    assert!("x".parse::<Selector>().is_err());
    assert!("a,x".parse::<Selector>().is_err());
}

#[test]
fn selector_reads_a_range_of_one_turn_as_that_turn() {
    let sel: Selector = "a:-1..-1".parse().unwrap();
    assert_eq!(sel, Selector::default());

    let sel: Selector = "a:3..3".parse().unwrap();
    assert_eq!(sel.range, PositionSpec::Single(Position::Absolute(3)));
    assert_eq!(sel.to_string(), "a:3");

    // Ends counted from different sides stay a range, even when they could
    // name the same turn.
    let sel: Selector = "a:3..-1".parse().unwrap();
    assert_eq!(
        sel.range,
        PositionSpec::Range(Some(Position::Absolute(3)), Some(Position::FromEnd(1)))
    );
}

#[test]
fn selector_rejects_zero_index() {
    assert_eq!(
        "a:0".parse::<Selector>().unwrap_err(),
        "turn numbers are 1-based; `0` is not a valid turn"
    );
    assert_eq!(
        "a:0..3".parse::<Selector>().unwrap_err(),
        "turn numbers are 1-based; `0` is not a valid turn"
    );
}

#[test]
fn selector_rejects_a_range_that_is_not_a_number() {
    assert_eq!(
        "a:2..x".parse::<Selector>().unwrap_err(),
        "invalid turn number `x`"
    );
    assert_eq!("a:".parse::<Selector>().unwrap_err(), "empty range");
}

#[test]
fn selector_rejects_empty_content() {
    assert!(",".parse::<Selector>().is_err());
}

#[test]
fn selector_roundtrip() {
    // Parsing a Selector's own Display form must yield an equal Selector.
    // We don't assert byte-for-byte equality with the input because the
    // Display form is canonical (e.g. content flags are ordered a,u,r,t).
    let cases = [
        "a:-1", "u,a:-1", "*:..", "a:2..4", "a:3..", "a:..5", "a:-3..", "a:2..-1",
    ];
    for input in cases {
        let sel: Selector = input.parse().unwrap();
        let round: Selector = sel.to_string().parse().unwrap();
        assert_eq!(sel, round, "roundtrip mismatch for {input}");
    }
}
