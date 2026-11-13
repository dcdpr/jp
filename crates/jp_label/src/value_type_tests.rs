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

/// `chrono`'s `%Y` specifier accepts signed and five-digit years, which sort
/// before four-digit years lexicographically (`'+' < '0'`).
#[test]
fn a_date_has_a_four_digit_unsigned_year() {
    assert!(!ValueType::Date.accepts("+10000-01-01"));
    assert!(!ValueType::Date.accepts("-0001-01-01"));
    assert!(!ValueType::Date.accepts("+2026-12-01"));
    assert!(!ValueType::Date.accepts("02026-12-01"));
    assert!(ValueType::Date.accepts("0001-01-01"));
    assert!(ValueType::Date.accepts("9999-12-31"));
}
