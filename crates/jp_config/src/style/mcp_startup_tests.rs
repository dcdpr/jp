use schematic::PartialConfig as _;
use test_log::test;

use super::*;

#[test]
fn defaults_show_after_four_seconds() {
    let p = PartialMcpStartupConfig::default_values(&())
        .unwrap()
        .unwrap();

    assert_eq!(p.show, Some(true));
    assert_eq!(p.delay_secs, Some(4));
    assert_eq!(p.interval_ms, Some(100));
}

#[test]
fn assign_fields_via_cli() {
    let mut p = PartialMcpStartupConfig::default();

    let kv = KvAssignment::try_from_cli("show", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.show, Some(false));

    let kv = KvAssignment::try_from_cli("delay_secs", "0").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.delay_secs, Some(0));

    let kv = KvAssignment::try_from_cli("interval_ms", "250").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.interval_ms, Some(250));
}
