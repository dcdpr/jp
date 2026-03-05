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
