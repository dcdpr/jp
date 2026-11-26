use reqwest::header::{HeaderName, HeaderValue};

use super::*;

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    headers
}

#[test]
fn test_no_unified_headers_is_empty() {
    let limits = UnifiedRateLimit::from_headers(&headers(&[("retry-after", "30")]));

    assert!(limits.is_empty());
    assert!(!limits.is_quota_rejection());
    assert!(!limits.is_rejected());
    assert_eq!(limits.warning(), None);
}

/// The per-window headers are abbreviated on the wire but named in full
/// everywhere else, so parsing normalizes them.
#[test]
fn test_window_utilization_is_parsed_under_its_full_name() {
    let limits = UnifiedRateLimit::from_headers(&headers(&[
        ("anthropic-ratelimit-unified-status", "allowed"),
        ("anthropic-ratelimit-unified-5h-utilization", "0.42"),
        ("anthropic-ratelimit-unified-5h-reset", "1780000000"),
        ("anthropic-ratelimit-unified-7d-utilization", "0.9"),
        ("anthropic-ratelimit-unified-7d-reset", "1780600000"),
    ]));

    assert_eq!(limits.windows, vec![
        WindowUtilization {
            claim: "five_hour".to_owned(),
            utilization: Some(0.42),
            reset: Some(1_780_000_000),
            surpassed_threshold: None,
        },
        WindowUtilization {
            claim: "seven_day".to_owned(),
            utilization: Some(0.9),
            reset: Some(1_780_600_000),
            surpassed_threshold: None,
        },
    ]);

    // No threshold was crossed, so there is nothing to warn about even
    // though one window is at 90%.
    assert_eq!(limits.warning(), None);
}

/// A window is only worth warning about once the provider says a threshold was
/// crossed; the fraction alone is not the signal.
#[test]
fn test_surpassed_threshold_marks_the_warning_window() {
    let limits = UnifiedRateLimit::from_headers(&headers(&[
        ("anthropic-ratelimit-unified-status", "allowed_warning"),
        ("anthropic-ratelimit-unified-5h-utilization", "0.2"),
        ("anthropic-ratelimit-unified-5h-reset", "1780000000"),
        ("anthropic-ratelimit-unified-7d-utilization", "0.75"),
        ("anthropic-ratelimit-unified-7d-reset", "1780600000"),
        ("anthropic-ratelimit-unified-7d-surpassed-threshold", "0.75"),
    ]));

    let warning = limits.warning().expect("the weekly window crossed one");
    assert_eq!(warning.claim, "seven_day");
    assert_eq!(warning.surpassed_threshold, Some(0.75));
}

/// A spent window reported on a response that still succeeded: paid extra usage
/// served it.
#[test]
fn test_rejected_status_is_readable_without_a_rejection() {
    let limits = UnifiedRateLimit::from_headers(&headers(&[
        ("anthropic-ratelimit-unified-status", "rejected"),
        (
            "anthropic-ratelimit-unified-representative-claim",
            "seven_day",
        ),
        ("anthropic-ratelimit-unified-reset", "1780600000"),
        ("anthropic-ratelimit-unified-overage-status", "allowed"),
    ]));

    assert!(limits.is_rejected());
    assert!(limits.is_quota_rejection());
    assert_eq!(limits.representative_claim.as_deref(), Some("seven_day"));
}

/// A rejection whose status says the allowance is still available was refused
/// for some other reason, even when it names a window.
#[test]
fn test_available_allowance_is_not_a_quota_rejection() {
    for status in ["allowed", "allowed_warning"] {
        let limits = UnifiedRateLimit::from_headers(&headers(&[
            ("anthropic-ratelimit-unified-status", status),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "five_hour",
            ),
            ("anthropic-ratelimit-unified-overage-status", "allowed"),
        ]));

        assert!(!limits.is_quota_rejection(), "status: {status}");
    }

    // Without a status header, the named window is enough.
    let limits = UnifiedRateLimit::from_headers(&headers(&[(
        "anthropic-ratelimit-unified-representative-claim",
        "five_hour",
    )]));
    assert!(limits.is_quota_rejection());
}

#[test]
fn test_overage_disabled_reason_is_parsed() {
    let limits = UnifiedRateLimit::from_headers(&headers(&[
        ("anthropic-ratelimit-unified-status", "rejected"),
        ("anthropic-ratelimit-unified-overage-status", "rejected"),
        (
            "anthropic-ratelimit-unified-overage-disabled-reason",
            "out_of_credits",
        ),
    ]));

    assert_eq!(
        limits.overage_disabled_reason.as_deref(),
        Some("out_of_credits")
    );
}
