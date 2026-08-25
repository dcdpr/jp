use std::time::Duration;

use test_log::test;

use super::*;

/// Build a resolved `RequestConfig` with the given idle timeout, leaving the
/// other fields at representative defaults.
fn request_config(stream_idle_timeout_secs: u32) -> RequestConfig {
    RequestConfig {
        max_retries: 5,
        base_backoff_ms: 1000,
        max_backoff_secs: 60,
        stream_idle_timeout_secs,
        max_response_bytes: MaxResponseBytes::default(),
        cache: CachePolicy::default(),
    }
}

#[test]
fn validate_rejects_sub_floor_idle_timeout() {
    let err = request_config(5).validate().unwrap_err();
    assert!(
        err.to_string().contains("stream_idle_timeout_secs"),
        "got: {err}"
    );
}

#[test]
fn validate_allows_disabled_and_at_floor_idle_timeout() {
    for secs in [0_u32, 10, 60] {
        assert!(request_config(secs).validate().is_ok(), "secs={secs}");
    }
}

#[test]
fn test_request_config_assign() {
    let mut p = PartialRequestConfig::default();

    let kv = KvAssignment::try_from_cli("max_retries", "10").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_retries, Some(10));

    let kv = KvAssignment::try_from_cli("base_backoff_ms", "2000").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.base_backoff_ms, Some(2000));

    let kv = KvAssignment::try_from_cli("max_backoff_secs", "120").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_backoff_secs, Some(120));

    let kv = KvAssignment::try_from_cli("stream_idle_timeout_secs", "20").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.stream_idle_timeout_secs, Some(20));

    let kv = KvAssignment::try_from_cli("max_response_bytes", "4096").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_response_bytes, Some(MaxResponseBytes::Bytes(4096)));

    let kv = KvAssignment::try_from_cli("max_response_bytes", "disabled").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_response_bytes, Some(MaxResponseBytes::Disabled));

    // `0` disables the ceiling, matching the sibling settings that spell "off"
    // that way.
    let kv = KvAssignment::try_from_cli("max_response_bytes", "0").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_response_bytes, Some(MaxResponseBytes::Disabled));
}

/// Every documented spelling works through the JSON assignment path too.
///
/// `KEY=false` arrives as a string and parses via `FromStr`, but `KEY:=false`
/// arrives as a JSON bool.
/// Accepting only the former makes an advertised value depend on which
/// assignment syntax the user reaches for.
#[test]
fn max_response_bytes_accepts_a_json_bool() {
    let mut p = PartialRequestConfig::default();

    let kv = KvAssignment::try_from_cli("max_response_bytes:", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_response_bytes, Some(MaxResponseBytes::Disabled));

    let kv = KvAssignment::try_from_cli("max_response_bytes:", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_response_bytes, Some(MaxResponseBytes::default()));

    // The number and string arms still work through the same helper.
    let kv = KvAssignment::try_from_cli("max_response_bytes:", "4096").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_response_bytes, Some(MaxResponseBytes::Bytes(4096)));

    let kv = KvAssignment::try_from_cli("max_response_bytes:", "\"disabled\"").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.max_response_bytes, Some(MaxResponseBytes::Disabled));
}

#[test]
fn max_response_bytes_round_trips_through_json() {
    for (value, expected) in [
        ("4096", MaxResponseBytes::Bytes(4096)),
        ("0", MaxResponseBytes::Disabled),
        ("\"disabled\"", MaxResponseBytes::Disabled),
        ("\"off\"", MaxResponseBytes::Disabled),
        ("false", MaxResponseBytes::Disabled),
    ] {
        let parsed: MaxResponseBytes = serde_json::from_str(value).expect(value);
        assert_eq!(parsed, expected, "parsing {value}");

        // Re-serializing and re-parsing must land on the same value, so a
        // stored conversation config keeps its meaning.
        let encoded = serde_json::to_string(&parsed).expect("serializes");
        let reparsed: MaxResponseBytes = serde_json::from_str(&encoded).expect(&encoded);
        assert_eq!(reparsed, expected, "round-tripping {value} via {encoded}");
    }
}

#[test]
fn max_response_bytes_rejects_a_nonsense_value() {
    let err = "banana".parse::<MaxResponseBytes>().unwrap_err();
    assert!(err.contains("expected a byte count"), "got: {err}");
}

/// `0` disables the ceiling on every input path.
///
/// A zero-byte ceiling would abort every response before its first event, so no
/// path may produce `Bytes(0)`.
#[test]
fn zero_disables_the_ceiling() {
    assert_eq!(MaxResponseBytes::from_bytes(0), MaxResponseBytes::Disabled);
    assert_eq!(
        "0".parse::<MaxResponseBytes>().unwrap(),
        MaxResponseBytes::Disabled
    );
    assert_eq!(
        serde_json::from_str::<MaxResponseBytes>("0").unwrap(),
        MaxResponseBytes::Disabled
    );
    assert_eq!(
        MaxResponseBytes::from_bytes(0).bytes(),
        None,
        "a disabled ceiling never yields a zero byte limit"
    );
}

#[test]
fn test_request_config_assign_object() {
    let mut p = PartialRequestConfig::default();

    let kv = KvAssignment::try_from_cli(
        ":",
        r#"{"max_retries":3,"base_backoff_ms":500,"max_backoff_secs":20}"#,
    )
    .unwrap();
    p.assign(kv).unwrap();

    assert_eq!(p.max_retries, Some(3));
    assert_eq!(p.base_backoff_ms, Some(500));
    assert_eq!(p.max_backoff_secs, Some(20));
}

#[test]
fn test_cache_policy_from_bool() {
    assert_eq!(CachePolicy::from(true), CachePolicy::Short);
    assert_eq!(CachePolicy::from(false), CachePolicy::Off);
}

#[test]
fn test_cache_policy_from_str() {
    assert_eq!("true".parse::<CachePolicy>(), Ok(CachePolicy::Short));
    assert_eq!("short".parse::<CachePolicy>(), Ok(CachePolicy::Short));
    assert_eq!("false".parse::<CachePolicy>(), Ok(CachePolicy::Off));
    assert_eq!("off".parse::<CachePolicy>(), Ok(CachePolicy::Off));
    assert_eq!("long".parse::<CachePolicy>(), Ok(CachePolicy::Long));
    assert_eq!(
        "10m".parse::<CachePolicy>(),
        Ok(CachePolicy::Custom(Duration::from_mins(10)))
    );
    assert_eq!(
        "1h".parse::<CachePolicy>(),
        Ok(CachePolicy::Custom(Duration::from_hours(1)))
    );
    assert!("invalid".parse::<CachePolicy>().is_err());
}

#[test]
fn test_cache_policy_serde_roundtrip() {
    // Serialize
    assert_eq!(serde_json::to_value(CachePolicy::Off).unwrap(), false);
    assert_eq!(serde_json::to_value(CachePolicy::Short).unwrap(), true);
    assert_eq!(serde_json::to_value(CachePolicy::Long).unwrap(), "long");
    assert_eq!(
        serde_json::to_value(CachePolicy::Custom(Duration::from_mins(10))).unwrap(),
        "10m"
    );

    // Deserialize from bool
    assert_eq!(
        serde_json::from_value::<CachePolicy>(true.into()).unwrap(),
        CachePolicy::Short
    );
    assert_eq!(
        serde_json::from_value::<CachePolicy>(false.into()).unwrap(),
        CachePolicy::Off
    );

    // Deserialize from string
    assert_eq!(
        serde_json::from_value::<CachePolicy>("off".into()).unwrap(),
        CachePolicy::Off
    );
    assert_eq!(
        serde_json::from_value::<CachePolicy>("short".into()).unwrap(),
        CachePolicy::Short
    );
    assert_eq!(
        serde_json::from_value::<CachePolicy>("long".into()).unwrap(),
        CachePolicy::Long
    );
    assert_eq!(
        serde_json::from_value::<CachePolicy>("10m".into()).unwrap(),
        CachePolicy::Custom(Duration::from_mins(10))
    );
}

#[test]
fn test_cache_policy_assign_kv() {
    let mut p = PartialRequestConfig::default();

    // Assign via string "off"
    let kv = KvAssignment::try_from_cli("cache", "off").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cache, Some(CachePolicy::Off));

    // Assign via string "long"
    let kv = KvAssignment::try_from_cli("cache", "long").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cache, Some(CachePolicy::Long));

    // Assign via duration string
    let kv = KvAssignment::try_from_cli("cache", "10m").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cache, Some(CachePolicy::Custom(Duration::from_mins(10))));

    // Assign via JSON bool
    let kv = KvAssignment::try_from_cli("cache:", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cache, Some(CachePolicy::Off));

    let kv = KvAssignment::try_from_cli("cache:", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cache, Some(CachePolicy::Short));
}

#[test]
fn test_cache_policy_assign_in_object() {
    let mut p = PartialRequestConfig::default();

    let kv = KvAssignment::try_from_cli(":", r#"{"cache":false}"#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cache, Some(CachePolicy::Off));

    let kv = KvAssignment::try_from_cli(":", r#"{"cache":"long"}"#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.cache, Some(CachePolicy::Long));
}
