use std::time::Duration;

use super::ExpirationDuration;

#[test]
fn parse_now() {
    let dur: ExpirationDuration = "now".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_now_case_insensitive() {
    let dur: ExpirationDuration = "NOW".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);

    let dur: ExpirationDuration = "Now".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_humantime_duration() {
    let dur: ExpirationDuration = "1h".parse().unwrap();
    assert_eq!(dur.0, Duration::from_hours(1));

    let dur: ExpirationDuration = "30m".parse().unwrap();
    assert_eq!(dur.0, Duration::from_mins(30));

    let dur: ExpirationDuration = "0s".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_invalid() {
    let result = "not-a-duration".parse::<ExpirationDuration>();
    assert!(result.is_err());
}
