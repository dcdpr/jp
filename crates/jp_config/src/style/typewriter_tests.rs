use std::time::Duration;

use schematic::PartialConfig as _;

use super::*;
use crate::assignment::KvAssignment;

#[test]
fn deserialize_humantime_strings() {
    let toml = r#"
        text_delay = "5ms"
        code_delay = "500us"
        max_latency = "200ms"
    "#;

    let partial: PartialTypewriterConfig = toml::from_str(toml).unwrap();

    assert_eq!(
        partial.text_delay,
        Some(DelayDuration(Duration::from_millis(5)))
    );
    assert_eq!(
        partial.code_delay,
        Some(DelayDuration(Duration::from_micros(500)))
    );
    assert_eq!(
        partial.max_latency,
        Some(DelayDuration(Duration::from_millis(200)))
    );
}

#[test]
fn deserialize_rejects_unknown_unit() {
    let toml = r#"text_delay = "5notaunit""#;
    let err = toml::from_str::<PartialTypewriterConfig>(toml).unwrap_err();
    assert!(
        err.to_string().contains("text_delay"),
        "error should mention field, got: {err}"
    );
}

#[test]
fn serialize_round_trips_through_humantime() {
    let dur = DelayDuration(Duration::from_millis(250));
    let json = serde_json::to_string(&dur).unwrap();
    assert_eq!(json, r#""250ms""#);

    let parsed: DelayDuration = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, dur);
}

#[test]
fn deserialize_accepts_legacy_secs_nanos_map() {
    // Pre-humantime conversations serialized `DelayDuration` using `Duration`'s
    // default `{secs, nanos}` shape. Reading them must keep working until they
    // get rewritten on the next save.
    let json = r#"{"secs":0,"nanos":3000000}"#;
    let parsed: DelayDuration = serde_json::from_str(json).unwrap();
    assert_eq!(parsed, DelayDuration(Duration::from_millis(3)));

    let json = r#"{"secs":1,"nanos":500000}"#;
    let parsed: DelayDuration = serde_json::from_str(json).unwrap();
    assert_eq!(parsed, DelayDuration(Duration::new(1, 500_000)));
}

#[test]
fn deserialize_legacy_map_ignores_unknown_keys() {
    let json = r#"{"secs":0,"nanos":1000,"extra":"ignored"}"#;
    let parsed: DelayDuration = serde_json::from_str(json).unwrap();
    assert_eq!(parsed, DelayDuration(Duration::from_micros(1)));
}

#[test]
fn assign_via_kv_uses_humantime() {
    let mut partial = PartialTypewriterConfig::default();

    let kv = KvAssignment::try_from_cli("text_delay", "200ms").unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(
        partial.text_delay,
        Some(DelayDuration(Duration::from_millis(200)))
    );
}

#[test]
fn defaults_resolve_to_expected_durations() {
    let defaults = PartialTypewriterConfig::default_values(&())
        .unwrap()
        .unwrap();

    assert_eq!(
        defaults.text_delay,
        Some(DelayDuration(Duration::from_millis(3)))
    );
    assert_eq!(
        defaults.code_delay,
        Some(DelayDuration(Duration::from_micros(500)))
    );
    assert_eq!(defaults.max_latency, Some(DelayDuration(Duration::ZERO)));
}
