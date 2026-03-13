use std::time::Duration;

use schematic::{Config as _, PartialConfig as _};
use test_log::test;

use super::*;

#[test]
fn test_request_config_defaults() {
    let partial = PartialRequestConfig::default_values(&()).unwrap().unwrap();
    let config = RequestConfig::from_partial(partial, vec![]).expect("valid config");

    assert_eq!(config.max_retries, 5);
    assert_eq!(config.base_backoff_ms, 1000);
    assert_eq!(config.max_backoff_secs, 60);
    assert_eq!(config.cache, CachePolicy::Short);
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
}

#[test]
fn test_request_config_assign_object() {
    let mut p = PartialRequestConfig::default();

    let kv = KvAssignment::try_from_cli(
        ":",
        r#"{"max_retries":3,"base_backoff_ms":500,"max_backoff_secs":30}"#,
    )
    .unwrap();
    p.assign(kv).unwrap();

    assert_eq!(p.max_retries, Some(3));
    assert_eq!(p.base_backoff_ms, Some(500));
    assert_eq!(p.max_backoff_secs, Some(30));
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
