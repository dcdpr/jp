use super::*;

#[test]
fn selector_default_is_last_assistant() {
    let sel: Selector = "".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, Range::last());
}

#[test]
fn selector_bare_content_defaults_range_to_last_turn() {
    let sel: Selector = "a".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, Range::last());
}

#[test]
fn selector_parses_combined_content() {
    let sel: Selector = "u,a,r,t:..".parse().unwrap();
    assert_eq!(sel.content, Content::all());
    assert_eq!(sel.range, Range::all());
}

#[test]
fn selector_star_is_all_content() {
    let sel: Selector = "*:..".parse().unwrap();
    assert_eq!(sel.content, Content::all());
}

#[test]
fn selector_parses_negative_shorthand() {
    let sel: Selector = "a:-3".parse().unwrap();
    assert_eq!(sel.range.start, Some(-3));
    assert_eq!(sel.range.end, None);
}

#[test]
fn selector_parses_positive_shorthand() {
    let sel: Selector = "a:5".parse().unwrap();
    assert_eq!(sel.range.start, Some(5));
    assert_eq!(sel.range.end, Some(5));
}

#[test]
fn selector_parses_explicit_range() {
    let sel: Selector = "a:2..4".parse().unwrap();
    assert_eq!(sel.range.start, Some(2));
    assert_eq!(sel.range.end, Some(4));
}

#[test]
fn selector_bare_negative_index_is_range_only() {
    // `-1` (no colon) means "default content, last turn" — same as `a:-1`.
    let sel: Selector = "-1".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range.start, Some(-1));
    assert_eq!(sel.range.end, None);
}

#[test]
fn selector_bare_positive_index_is_range_only() {
    let sel: Selector = "5".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range.start, Some(5));
    assert_eq!(sel.range.end, Some(5));
}

#[test]
fn selector_bare_full_range_is_range_only() {
    let sel: Selector = "..".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range, Range::all());
}

#[test]
fn selector_bare_explicit_range_is_range_only() {
    let sel: Selector = "5..-3".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range.start, Some(5));
    assert_eq!(sel.range.end, Some(-3));
}

#[test]
fn selector_colon_prefix_uses_default_content() {
    // `:-1` is the long-form equivalent of `-1`.
    let sel: Selector = ":-1".parse().unwrap();
    assert_eq!(sel.content, Content::assistant_only());
    assert_eq!(sel.range.start, Some(-1));
    assert_eq!(sel.range.end, None);
}

#[test]
fn selector_parses_open_ended_ranges() {
    let sel: Selector = "a:3..".parse().unwrap();
    assert_eq!(sel.range.start, Some(3));
    assert_eq!(sel.range.end, None);

    let sel: Selector = "a:..5".parse().unwrap();
    assert_eq!(sel.range.start, None);
    assert_eq!(sel.range.end, Some(5));
}

#[test]
fn selector_rejects_unknown_content_flag() {
    assert!("x".parse::<Selector>().is_err());
    assert!("a,x".parse::<Selector>().is_err());
}

#[test]
fn selector_rejects_zero_index() {
    assert!("a:0".parse::<Selector>().is_err());
    assert!("a:0..3".parse::<Selector>().is_err());
}

#[test]
fn selector_rejects_empty_content() {
    assert!(",".parse::<Selector>().is_err());
}

#[test]
fn range_resolve_last_turn() {
    assert_eq!(Range::last().resolve(5), Some((4, 5)));
    assert_eq!(Range::last().resolve(1), Some((0, 1)));
    assert_eq!(Range::last().resolve(0), None);
}

#[test]
fn range_resolve_negative_window() {
    let range = Range {
        start: Some(-3),
        end: None,
    };
    assert_eq!(range.resolve(10), Some((7, 10)));
    // Windows that would extend before the start are clamped.
    assert_eq!(range.resolve(2), Some((0, 2)));
}

#[test]
fn range_resolve_positive_window() {
    let range = Range {
        start: Some(2),
        end: Some(4),
    };
    assert_eq!(range.resolve(10), Some((1, 4)));
    // Bounds past the end are clamped.
    assert_eq!(range.resolve(3), Some((1, 3)));
    // Start past the end returns None.
    assert_eq!(range.resolve(1), None);
}

#[test]
fn range_resolve_all() {
    assert_eq!(Range::all().resolve(5), Some((0, 5)));
}

#[test]
fn selector_roundtrip() {
    // Parsing a Selector's own Display form must yield an equal Selector.
    // We don't assert byte-for-byte equality with the input because the
    // Display form is canonical (e.g. content flags are ordered a,u,r,t).
    let cases = ["a:-1", "u,a:-1", "*:..", "a:2..4", "a:3..", "a:..5"];
    for input in cases {
        let sel: Selector = input.parse().unwrap();
        let round: Selector = sel.to_string().parse().unwrap();
        assert_eq!(sel, round, "roundtrip mismatch for {input}");
    }
}
