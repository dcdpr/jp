use super::*;

#[test]
fn reports_absolute_offsets_when_searching_from_a_position() {
    let matcher = FancyMatcher::new("needle").unwrap();

    let found = matcher.find_at(b"needle and needle", 7).unwrap().unwrap();

    assert_eq!((found.start(), found.end()), (11, 17));
}

#[test]
fn lookbehind_reaches_behind_the_start_position() {
    let matcher = FancyMatcher::new("(?<=let )needle").unwrap();

    // The match begins at 4, so a search from 4 only succeeds if the bytes
    // before it are still visible to the lookbehind.
    let found = matcher.find_at(b"let needle", 4).unwrap().unwrap();

    assert_eq!((found.start(), found.end()), (4, 10));
}

#[test]
fn caret_anchors_to_a_line_start_within_the_buffer() {
    let matcher = FancyMatcher::new("^beta").unwrap();

    let found = matcher.find_at(b"alpha\nbeta\n", 0).unwrap().unwrap();

    assert_eq!((found.start(), found.end()), (6, 10));
}

#[test]
fn offsets_stay_absolute_in_a_haystack_holding_invalid_utf8() {
    let matcher = FancyMatcher::new("needle").unwrap();

    // The latin-1 byte occupies one byte, so the match starts at 5. Reporting
    // it anywhere else would make the searcher slice the wrong line.
    let found = matcher.find_at(b"caf\xe9 needle", 0).unwrap().unwrap();

    assert_eq!((found.start(), found.end()), (5, 11));
}

#[test]
fn a_pattern_that_cannot_match_returns_none_rather_than_an_error() {
    let matcher = FancyMatcher::new("needle").unwrap();

    assert!(matcher.find_at(b"nothing here", 0).unwrap().is_none());
}
