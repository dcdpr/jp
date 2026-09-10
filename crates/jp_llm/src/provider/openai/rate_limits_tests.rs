use reqwest::header::{HeaderName, HeaderValue};

use super::*;

/// The header set a real `200` from the subscription endpoint carries.
///
/// Captured from a live request on a `ChatGPT` Pro account, including the two
/// quirks a parser has to survive: an empty `secondary-reset-at`, and a
/// per-model family (`bengalfox`) alongside the account-wide one.
const LIVE_HEADERS: &[(&str, &str)] = &[
    ("x-codex-active-limit", "premium"),
    ("x-codex-plan-type", "prolite"),
    ("x-codex-primary-used-percent", "0"),
    ("x-codex-secondary-used-percent", "0"),
    ("x-codex-primary-window-minutes", "10080"),
    ("x-codex-primary-over-secondary-limit-percent", "0"),
    ("x-codex-secondary-window-minutes", "0"),
    ("x-codex-primary-reset-after-seconds", "604800"),
    ("x-codex-secondary-reset-after-seconds", "0"),
    ("x-codex-primary-reset-at", "1789571252"),
    ("x-codex-secondary-reset-at", ""),
    ("x-codex-credits-has-credits", "False"),
    ("x-codex-credits-balance", "0"),
    ("x-codex-credits-unlimited", "False"),
    ("x-codex-bengalfox-primary-used-percent", "0"),
    ("x-codex-bengalfox-secondary-used-percent", "0"),
    ("x-codex-bengalfox-primary-window-minutes", "300"),
    (
        "x-codex-bengalfox-primary-over-secondary-limit-percent",
        "0",
    ),
    ("x-codex-bengalfox-secondary-window-minutes", "10080"),
    ("x-codex-bengalfox-primary-reset-after-seconds", "18000"),
    ("x-codex-bengalfox-secondary-reset-after-seconds", "604800"),
    ("x-codex-bengalfox-primary-reset-at", "1788984452"),
    ("x-codex-bengalfox-secondary-reset-at", "1789571252"),
    ("x-codex-bengalfox-limit-name", "GPT-5.3-Codex-Spark"),
];

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();

    for (name, value) in pairs {
        map.insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }

    map
}

#[test]
fn test_parse_all_reads_both_families_from_a_live_response() {
    let snapshots = parse_all(&headers(LIVE_HEADERS));

    let ids: Vec<&str> = snapshots.iter().map(|s| s.limit_id.as_str()).collect();
    assert_eq!(ids, vec!["bengalfox", "codex"]);
}

#[test]
#[expect(clippy::float_cmp, reason = "the header's value is an exact literal")]
fn test_account_family_reports_only_its_primary_window() {
    let snapshots = parse_all(&headers(LIVE_HEADERS));
    let account = snapshots.iter().find(|s| s.limit_id == "codex").unwrap();

    let primary = account.primary.as_ref().unwrap();
    assert_eq!(primary.used_percent, 0.0);
    assert_eq!(primary.window_minutes, Some(10080));
    assert_eq!(primary.name("primary"), "7-day limit");

    // Every secondary value is zero or blank, so there is no second window on
    // this plan rather than an empty one.
    assert_eq!(account.secondary, None);
    assert_eq!(account.limit_name, None);
    assert_eq!(account.scope(), "account");
}

#[test]
fn test_model_family_reports_its_name_and_both_windows() {
    let snapshots = parse_all(&headers(LIVE_HEADERS));
    let model = snapshots
        .iter()
        .find(|s| s.limit_id == "bengalfox")
        .unwrap();

    assert_eq!(model.limit_name.as_deref(), Some("GPT-5.3-Codex-Spark"));
    assert_eq!(model.scope(), "gpt-5.3-codex-spark");
    assert_eq!(
        model.primary.as_ref().unwrap().name("primary"),
        "5-hour limit"
    );
    assert_eq!(
        model.secondary.as_ref().unwrap().name("secondary"),
        "7-day limit"
    );
}

#[test]
fn test_reset_prefers_the_relative_header() {
    // `-reset-after-seconds` is 300s while `-reset-at` is an hour in the past;
    // trusting the absolute value would report a window that already reset.
    let map = headers(&[
        ("x-codex-primary-used-percent", "50"),
        ("x-codex-primary-window-minutes", "300"),
        ("x-codex-primary-reset-after-seconds", "300"),
        ("x-codex-primary-reset-at", "1000000000"),
    ]);

    let snapshot = parse_all(&map)
        .into_iter()
        .find(|s| s.limit_id == "codex")
        .unwrap();
    let resets_at = snapshot.primary.unwrap().resets_at.unwrap();

    assert!(
        resets_at > Utc::now(),
        "expected a future reset, got {resets_at}"
    );
}

#[test]
fn test_reset_falls_back_to_the_absolute_header() {
    let map = headers(&[
        ("x-codex-primary-used-percent", "50"),
        ("x-codex-primary-reset-after-seconds", "0"),
        ("x-codex-primary-reset-at", "1789571252"),
    ]);

    let snapshot = parse_all(&map)
        .into_iter()
        .find(|s| s.limit_id == "codex")
        .unwrap();

    assert_eq!(
        snapshot.primary.unwrap().resets_at,
        DateTime::from_timestamp(1_789_571_252, 0)
    );
}

#[test]
fn test_a_header_set_without_the_family_yields_nothing() {
    let snapshots = parse_all(&headers(&[("x-openai-proxy-wasm", "v0.1")]));

    assert!(snapshots.is_empty(), "unexpected snapshots: {snapshots:?}");
}

#[test]
fn test_spent_window_is_reported() {
    let map = headers(&[
        ("x-codex-primary-used-percent", "100"),
        ("x-codex-primary-window-minutes", "10080"),
    ]);

    let snapshot = parse_all(&map).into_iter().next().unwrap();

    assert!(snapshot.spent().is_some());
    assert!(snapshot.warning().is_none());
}

#[test]
#[expect(clippy::float_cmp, reason = "the header's value is an exact literal")]
fn test_warning_picks_the_fullest_unspent_window() {
    let map = headers(&[
        ("x-codex-primary-used-percent", "85"),
        ("x-codex-primary-window-minutes", "300"),
        ("x-codex-secondary-used-percent", "95"),
        ("x-codex-secondary-window-minutes", "10080"),
    ]);

    let snapshot = parse_all(&map).into_iter().next().unwrap();
    let warning = snapshot.warning().unwrap();

    assert_eq!(warning.used_percent, 95.0);
    assert_eq!(warning.name("secondary"), "7-day limit");
}

#[test]
fn test_a_window_below_the_threshold_does_not_warn() {
    let map = headers(&[
        ("x-codex-primary-used-percent", "79.9"),
        ("x-codex-primary-window-minutes", "300"),
    ]);

    let snapshot = parse_all(&map).into_iter().next().unwrap();

    assert!(snapshot.warning().is_none());
    assert!(snapshot.spent().is_none());
}

#[test]
fn test_window_name_handles_an_unreported_length() {
    let window = Window {
        used_percent: 10.0,
        window_minutes: None,
        resets_at: None,
    };

    assert_eq!(window.name("primary"), "primary limit");
}

#[test]
fn test_window_name_handles_a_sub_hour_length() {
    let window = Window {
        used_percent: 10.0,
        window_minutes: Some(45),
        resets_at: None,
    };

    assert_eq!(window.name("primary"), "45-minute limit");
}
