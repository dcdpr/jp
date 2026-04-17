use chrono::{TimeZone as _, Utc};

use super::*;

#[test]
fn parse_relative_duration_weeks() {
    let before = Utc::now();
    let t: TimeThreshold = "3w".parse().unwrap();
    let after = Utc::now();

    // 3 weeks ago, give or take a second for test execution.
    let expected_approx = before - chrono::Duration::weeks(3);
    assert!(
        *t >= expected_approx - chrono::Duration::seconds(1)
            && *t <= after - chrono::Duration::weeks(3) + chrono::Duration::seconds(1)
    );
}

#[test]
fn parse_relative_duration_days() {
    let t: TimeThreshold = "30d".parse().unwrap();
    let diff = Utc::now() - *t;
    // Should be roughly 30 days.
    assert!((diff.num_days() - 30).abs() <= 1);
}

#[test]
fn parse_relative_duration_hours() {
    let t: TimeThreshold = "6h".parse().unwrap();
    let diff = Utc::now() - *t;
    assert!((diff.num_hours() - 6).abs() <= 1);
}

#[test]
fn parse_rfc3339_datetime() {
    let t: TimeThreshold = "2026-01-15T10:30:00Z".parse().unwrap();
    let expected = Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 0).unwrap();
    assert_eq!(*t, expected);
}

#[test]
fn parse_date_only() {
    let t: TimeThreshold = "2026-01-01".parse().unwrap();
    let expected = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    assert_eq!(*t, expected);
}

#[test]
fn parse_invalid_input() {
    let result = "not-a-time".parse::<TimeThreshold>();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid time threshold"));
}

#[test]
fn deref_to_datetime() {
    let t: TimeThreshold = "2026-06-15".parse().unwrap();
    let dt: &chrono::DateTime<Utc> = &t;
    assert_eq!(
        dt.date_naive(),
        chrono::NaiveDate::from_ymd_opt(2026, 6, 15).unwrap()
    );
}

#[test]
fn into_datetime() {
    let t: TimeThreshold = "2026-06-15".parse().unwrap();
    let dt: chrono::DateTime<Utc> = t.into();
    assert_eq!(dt, *t);
}

#[test]
fn from_datetime() {
    let dt = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
    let t: TimeThreshold = dt.into();
    assert_eq!(*t, dt);
}
