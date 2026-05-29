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
    });
    assert!(!required.optional());

    let optional = McpProviderConfig::Stdio(StdioConfig {
        command: "echo".into(),
        arguments: vec![],
        variables: vec![],
        checksum: None,
        optional: true,
    });
    assert!(optional.optional());
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
