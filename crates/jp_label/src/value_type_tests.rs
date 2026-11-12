use super::*;

#[test]
fn a_date_is_a_zero_padded_calendar_date() {
    assert!(ValueType::Date.accepts("2026-12-01"));
    assert!(ValueType::Date.accepts("2028-02-29"));

    assert!(!ValueType::Date.accepts("2026-02-29"));
    assert!(!ValueType::Date.accepts("2026-13-01"));
    assert!(!ValueType::Date.accepts("2026-1-5"));
    assert!(!ValueType::Date.accepts("2026-12-01T00:00:00Z"));
    assert!(!ValueType::Date.accepts(" 2026-12-01"));
    assert!(!ValueType::Date.accepts("1w"));
    assert!(!ValueType::Date.accepts(""));
}
