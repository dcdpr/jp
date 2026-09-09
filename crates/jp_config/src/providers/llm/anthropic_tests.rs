use test_log::test;

use super::*;
use crate::{AppConfig, assignment::KvAssignment};

#[test]
fn test_auth_entry_from_str() {
    let cases = [
        ("api_key", Ok(AuthEntry::ApiKey)),
        ("profile", Ok(AuthEntry::Profile(None))),
        (
            "profile:personal",
            Ok(AuthEntry::Profile(Some("personal".to_owned()))),
        ),
        // Profile names are case-sensitive and preserved verbatim.
        (
            "profile:Work",
            Ok(AuthEntry::Profile(Some("Work".to_owned()))),
        ),
        ("profile:", Err(())),
        ("API_KEY", Err(())),
        ("token", Err(())),
        ("", Err(())),
    ];

    for (input, expected) in cases {
        let actual = input.parse::<AuthEntry>();
        match expected {
            Ok(entry) => assert_eq!(actual.unwrap(), entry, "input: {input:?}"),
            Err(()) => assert!(actual.is_err(), "input: {input:?}"),
        }
    }
}

#[test]
fn test_auth_entry_display_roundtrip() {
    let entries = [
        AuthEntry::ApiKey,
        AuthEntry::Profile(None),
        AuthEntry::Profile(Some("personal".to_owned())),
    ];

    for entry in entries {
        let rendered = entry.to_string();
        assert_eq!(rendered.parse::<AuthEntry>().unwrap(), entry);
    }
}

#[test]
fn test_auth_entry_serde() {
    let entry: AuthEntry = serde_json::from_str(r#""profile:work""#).unwrap();
    assert_eq!(entry, AuthEntry::Profile(Some("work".to_owned())));
    assert_eq!(serde_json::to_string(&entry).unwrap(), r#""profile:work""#);

    let error = serde_json::from_str::<AuthEntry>(r#""bogus""#).unwrap_err();
    assert!(error.to_string().contains("unrecognized auth chain entry"));
}

#[test]
fn test_default_auth_chain_is_api_key() {
    let config = AppConfig::new_test();
    assert_eq!(config.providers.llm.anthropic.auth, vec![AuthEntry::ApiKey]);
}

#[test]
fn test_validate_rejects_empty_auth_chain() {
    let mut config = AppConfig::new_test();
    config.providers.llm.anthropic.auth = vec![];

    let error = config.providers.llm.anthropic.validate().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("must contain at least one entry")
    );
}

#[test]
fn test_validate_rejects_duplicate_auth_entries() {
    let mut config = AppConfig::new_test();
    config.providers.llm.anthropic.auth = vec![
        AuthEntry::Profile(Some("work".to_owned())),
        AuthEntry::ApiKey,
        AuthEntry::Profile(Some("work".to_owned())),
    ];

    let error = config.providers.llm.anthropic.validate().unwrap_err();
    assert!(error.to_string().contains("duplicate entry profile:work"));
}

#[test]
fn test_assign_auth_chain() {
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = r#"auth:=["profile:personal","api_key"]"#.parse().unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(
        partial.auth,
        Some(vec![
            AuthEntry::Profile(Some("personal".to_owned())),
            AuthEntry::ApiKey,
        ])
    );
}

#[test]
fn test_assign_auth_chain_from_comma_separated_string() {
    // The string form used by `JP_CFG_PROVIDERS_LLM_ANTHROPIC_AUTH` and
    // plain `--cfg key=value` assignments.
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = "auth=profile:personal,api_key".parse().unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(
        partial.auth,
        Some(vec![
            AuthEntry::Profile(Some("personal".to_owned())),
            AuthEntry::ApiKey,
        ])
    );
}

#[test]
fn test_assign_auth_chain_rejects_unrecognized_entry() {
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = r#"auth:=["bogus"]"#.parse().unwrap();

    assert!(partial.assign(kv).is_err());
}

#[test]
fn test_assign_beta_headers() {
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = r#"beta_headers:=["context-editing-2025-06-27"]"#.parse().unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(
        partial.beta_headers,
        Some(vec!["context-editing-2025-06-27".to_owned()])
    );
}
