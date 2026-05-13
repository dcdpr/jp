use chrono::{DateTime, TimeZone as _, Utc};
use clap::Parser;
use jp_conversation::ConversationId;

use super::*;
use crate::cmd::conversation_id::PositionalIds;

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
fn parse_conversation_id() {
    let dt = Utc.with_ymd_and_hms(2026, 1, 15, 10, 30, 0).unwrap();
    let id = ConversationId::try_from(dt).unwrap();

    let t: TimeThreshold = id.to_string().parse().unwrap();
    assert_eq!(*t, dt);
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

// --- CreationRange ----------------------------------------------------------

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

fn ts(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + chrono::Duration::seconds(secs)
}

#[test]
fn creation_range_is_set() {
    assert!(!CreationRange::default().is_set());
    assert!(
        CreationRange {
            from: Some(ts(1000).into()),
            until: None,
        }
        .is_set()
    );
    assert!(
        CreationRange {
            from: None,
            until: Some(ts(1000).into()),
        }
        .is_set()
    );
}

#[test]
fn creation_range_from_inclusive() {
    let range = CreationRange {
        from: Some(ts(1000).into()),
        until: None,
    };

    // Strictly before: excluded.
    assert!(!range.matches(make_id(999)));
    // Equal: included (>=).
    assert!(range.matches(make_id(1000)));
    // After: included.
    assert!(range.matches(make_id(1001)));
}

#[test]
fn creation_range_until_exclusive() {
    let range = CreationRange {
        from: None,
        until: Some(ts(2000).into()),
    };

    assert!(range.matches(make_id(1999)));
    // Equal: excluded (<).
    assert!(!range.matches(make_id(2000)));
    assert!(!range.matches(make_id(2001)));
}

#[test]
fn creation_range_half_open() {
    let range = CreationRange {
        from: Some(ts(1000).into()),
        until: Some(ts(2000).into()),
    };

    assert!(!range.matches(make_id(999)));
    assert!(range.matches(make_id(1000)));
    assert!(range.matches(make_id(1500)));
    assert!(!range.matches(make_id(2000)));
    assert!(!range.matches(make_id(2500)));
}

#[test]
fn creation_range_unset_accepts_everything() {
    let range = CreationRange::default();
    assert!(range.matches(make_id(0)));
    assert!(range.matches(make_id(1_000_000_000)));
}

// --- Clap integration -------------------------------------------------------
//
// `CreationRange` is only useful when flattened next to a `PositionalIds`
// that provides the `id` arg referenced by `conflicts_with`. These tests
// lock in the parse behaviour so a future refactor can't silently break the
// conflict, the duration syntax, or the conversation-ID shorthand.

#[derive(Debug, Parser)]
#[command(name = "test-creation-range")]
struct RangeWithIds {
    #[command(flatten)]
    _target: PositionalIds<true, true>,

    #[command(flatten)]
    range: CreationRange,
}

#[test]
fn clap_parses_from_and_until() {
    let cmd =
        RangeWithIds::try_parse_from(["test-creation-range", "--from", "3w", "--until", "1d"])
            .unwrap();
    assert!(cmd.range.from.is_some());
    assert!(cmd.range.until.is_some());
    assert!(cmd.range.is_set());
}

#[test]
fn clap_conflicts_range_with_positional_id() {
    let id = ConversationId::try_from(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()).unwrap();
    let err =
        RangeWithIds::try_parse_from(["test-creation-range", &id.to_string(), "--from", "3w"])
            .unwrap_err();
    assert!(
        err.to_string().contains("cannot be used with"),
        "expected clap conflict error, got: {err}"
    );
}
