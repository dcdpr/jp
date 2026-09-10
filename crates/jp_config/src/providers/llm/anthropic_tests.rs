use schematic::PartialConfig as _;
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
fn test_assign_auth_chain_null_clears_to_none() {
    let mut partial = partial_with_auth(&[AuthEntry::ApiKey]);

    let kv: KvAssignment = "auth:=null".parse().unwrap();
    partial.assign(kv).unwrap();

    // `None` and `Some([])` merge differently: `None` lets a later layer's
    // chain land verbatim, while an empty chain replaces it with a value
    // validation then rejects.
    assert_eq!(partial.auth, None);
}

/// A chain holding `before`, as one config layer's partial.
fn partial_with_auth(chain: &[AuthEntry]) -> PartialAnthropicConfig {
    PartialAnthropicConfig {
        auth: Some(chain.to_vec()),
        ..PartialAnthropicConfig::default()
    }
}

/// Assert that the delta between two chains folds back onto the first.
///
/// Order is part of the assertion: the chain is a fallback order, so a delta
/// that reproduces the set but not the sequence silently changes which
/// credential pays for the request.
fn assert_auth_delta_law(before: &[AuthEntry], after: &[AuthEntry]) {
    let prev = partial_with_auth(before);
    let next = partial_with_auth(after);

    let delta = prev.delta(next.clone());

    let mut folded = prev;
    folded
        .merge(&(), delta)
        .expect("folding a delta cannot fail");

    assert_eq!(
        folded.auth, next.auth,
        "{before:?} -> {after:?} did not fold back to the new chain"
    );
}

#[test]
fn test_auth_delta_law_holds_for_a_removed_entry() {
    assert_auth_delta_law(
        &[
            AuthEntry::Profile(Some("work".to_owned())),
            AuthEntry::ApiKey,
        ],
        &[AuthEntry::Profile(Some("work".to_owned()))],
    );
}

#[test]
fn test_auth_delta_law_holds_for_a_reordered_chain() {
    assert_auth_delta_law(
        &[
            AuthEntry::Profile(Some("work".to_owned())),
            AuthEntry::ApiKey,
        ],
        &[
            AuthEntry::ApiKey,
            AuthEntry::Profile(Some("work".to_owned())),
        ],
    );
}

#[test]
fn test_auth_delta_law_holds_for_an_added_entry() {
    assert_auth_delta_law(&[AuthEntry::Profile(Some("work".to_owned()))], &[
        AuthEntry::Profile(Some("work".to_owned())),
        AuthEntry::ApiKey,
    ]);
}

#[test]
fn test_auth_delta_is_empty_for_an_unchanged_chain() {
    let prev = partial_with_auth(&[AuthEntry::ApiKey]);

    let delta = prev.delta(partial_with_auth(&[AuthEntry::ApiKey]));

    assert_eq!(delta.auth, None);
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
