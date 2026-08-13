use schematic::PartialConfig as _;
use test_log::test;

use super::*;
use crate::assignment::KvAssignment;

#[test]
fn stdio_optional_defaults_to_false() {
    let config = StdioConfig {
        command: "echo".into(),
        arguments: vec![],
        variables: vec![],
        checksum: None,
        optional: bool::default(),
        startup_timeout_secs: 60,
    };

    assert!(!config.optional);
}

#[test]
fn mcp_provider_optional_reports_stdio_flag() {
    let required = McpProviderConfig::Stdio(StdioConfig {
        command: "echo".into(),
        arguments: vec![],
        variables: vec![],
        checksum: None,
        optional: false,
        startup_timeout_secs: 60,
    });
    assert!(!required.optional());

    let optional = McpProviderConfig::Stdio(StdioConfig {
        command: "echo".into(),
        arguments: vec![],
        variables: vec![],
        checksum: None,
        optional: true,
        startup_timeout_secs: 60,
    });
    assert!(optional.optional());
}

#[test]
fn startup_timeout_defaults_to_60_seconds() {
    let p = PartialStdioConfig::default_values(&()).unwrap().unwrap();
    assert_eq!(p.startup_timeout_secs, Some(60));
}

#[test]
fn assign_startup_timeout_via_cli() {
    let mut p = PartialStdioConfig::default();
    assert_eq!(p.startup_timeout_secs, None);

    let kv = KvAssignment::try_from_cli("startup_timeout_secs", "300").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.startup_timeout_secs, Some(300));
}

#[test]
fn assign_optional_flag_via_cli() {
    let mut p = PartialStdioConfig::default();
    assert_eq!(p.optional, None);

    let kv = KvAssignment::try_from_cli("optional", "true").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.optional, Some(true));

    let kv = KvAssignment::try_from_cli("optional", "false").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.optional, Some(false));
}
