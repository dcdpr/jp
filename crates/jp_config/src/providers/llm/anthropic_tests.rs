use schematic::PartialConfig as _;
use test_log::test;

use super::*;
use crate::{AppConfig, assignment::KvAssignment};

#[test]
fn test_auth_entry_from_str() {
    let cases = [
        ("api_key", Ok(AuthEntry::ApiKey(None))),
        ("subscription", Ok(AuthEntry::Subscription(None))),
        // Each kind has a shorthand, meaning exactly the long form.
        ("api", Ok(AuthEntry::ApiKey(None))),
        ("sub", Ok(AuthEntry::Subscription(None))),
        (
            "subscription:personal",
            Ok(AuthEntry::Subscription(Some("personal".to_owned()))),
        ),
        (
            "sub:personal",
            Ok(AuthEntry::Subscription(Some("personal".to_owned()))),
        ),
        (
            "api_key:work",
            Ok(AuthEntry::ApiKey(Some("work".to_owned()))),
        ),
        ("api:work", Ok(AuthEntry::ApiKey(Some("work".to_owned())))),
        // Names are case-sensitive and preserved verbatim.
        (
            "subscription:Work",
            Ok(AuthEntry::Subscription(Some("Work".to_owned()))),
        ),
        // A trailing colon names nothing, which is a typo rather than a bare
        // kind.
        ("subscription:", Err(())),
        ("api_key:", Err(())),
        // A bare word is a credential name, whatever it says. Resolution is
        // where an unknown one is caught, and it reports what is configured.
        ("API_KEY", Ok(AuthEntry::Named("API_KEY".to_owned()))),
        ("token", Ok(AuthEntry::Named("token".to_owned()))),
        // The old spelling, which no longer names a kind. Written with a name
        // it is a kind that does not exist, and says so.
        ("profile:work", Err(())),
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
        AuthEntry::ApiKey(None),
        AuthEntry::ApiKey(Some("work".to_owned())),
        AuthEntry::Subscription(None),
        AuthEntry::Subscription(Some("personal".to_owned())),
    ];

    for entry in entries {
        let rendered = entry.to_string();
        assert_eq!(rendered.parse::<AuthEntry>().unwrap(), entry);
    }
}

/// A bare name may turn out to be a subscription, so a chain containing one has
/// to open the credential store to find out.
///
/// A provider deciding this by matching `Subscription` alone leaves every bare
/// name unresolvable, which is what a caller sees as `no credential named
/// 'personal'` while that credential sits in the store.
#[test]
fn test_a_bare_name_may_need_the_credential_store() {
    assert!(AuthEntry::Named("personal".to_owned()).may_need_store());
    assert!(AuthEntry::Subscription(None).may_need_store());
    assert!(AuthEntry::Subscription(Some("personal".to_owned())).may_need_store());

    // An API key is read from the environment, so the store is never involved.
    assert!(!AuthEntry::ApiKey(None).may_need_store());
    assert!(!AuthEntry::ApiKey(Some("work".to_owned())).may_need_store());
}

/// A shorthand is a way to type an entry, not a second way to store one.
#[test]
fn test_auth_entry_shorthands_are_written_back_in_full() {
    for (shorthand, canonical) in [
        ("api", "api_key"),
        ("api:work", "api_key:work"),
        ("sub", "subscription"),
        ("sub:personal", "subscription:personal"),
    ] {
        let entry: AuthEntry = shorthand.parse().unwrap();
        assert_eq!(entry.to_string(), canonical);
    }
}

#[test]
fn test_auth_entry_serde() {
    let entry: AuthEntry = serde_json::from_str(r#""subscription:work""#).unwrap();
    assert_eq!(entry, AuthEntry::Subscription(Some("work".to_owned())));
    assert_eq!(
        serde_json::to_string(&entry).unwrap(),
        r#""subscription:work""#
    );

    let error = serde_json::from_str::<AuthEntry>(r#""bogus:x""#).unwrap_err();
    assert!(error.to_string().contains("unrecognized auth chain entry"));
}

#[test]
fn test_default_auth_chain_is_api_key() {
    let config = AppConfig::new_test();
    assert_eq!(config.providers.llm.anthropic.auth, vec![
        AuthEntry::ApiKey(None)
    ]);
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
        AuthEntry::Subscription(Some("work".to_owned())),
        AuthEntry::ApiKey(None),
        AuthEntry::Subscription(Some("work".to_owned())),
    ];

    let error = config.providers.llm.anthropic.validate().unwrap_err();
    assert!(
        error
            .to_string()
            .contains("duplicate entry subscription:work")
    );
}

#[test]
fn test_assign_auth_chain() {
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = r#"auth:=["subscription:personal","api_key"]"#.parse().unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(
        partial.auth,
        Some(vec![
            AuthEntry::Subscription(Some("personal".to_owned())),
            AuthEntry::ApiKey(None),
        ])
    );
}

#[test]
fn test_assign_auth_chain_from_comma_separated_string() {
    // The string form used by `JP_CFG_PROVIDERS_LLM_ANTHROPIC_AUTH` and
    // plain `--cfg key=value` assignments.
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = "auth=subscription:personal,api_key".parse().unwrap();
    partial.assign(kv).unwrap();

    assert_eq!(
        partial.auth,
        Some(vec![
            AuthEntry::Subscription(Some("personal".to_owned())),
            AuthEntry::ApiKey(None),
        ])
    );
}

#[test]
fn test_assign_auth_chain_null_clears_to_none() {
    let mut partial = partial_with_auth(&[AuthEntry::ApiKey(None)]);

    let kv: KvAssignment = "auth:=null".parse().unwrap();
    partial.assign(kv).unwrap();

    // `None` and `Some([])` merge differently: `None` lets a later layer's
    // chain land verbatim, `Some([])` replaces it with an empty chain that
    // validation rejects.
    assert_eq!(partial.auth, None);
}

fn partial_with_auth(chain: &[AuthEntry]) -> PartialAnthropicConfig {
    PartialAnthropicConfig {
        auth: Some(chain.to_vec()),
        ..PartialAnthropicConfig::default()
    }
}

/// Assert that folding the delta between two chains onto `before` yields
/// `after`.
///
/// Order is part of the assertion: the chain is a fallback order, so a delta
/// that reproduces the entries but not their sequence silently changes which
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
            AuthEntry::Subscription(Some("work".to_owned())),
            AuthEntry::ApiKey(None),
        ],
        &[AuthEntry::Subscription(Some("work".to_owned()))],
    );
}

#[test]
fn test_auth_delta_law_holds_for_a_reordered_chain() {
    assert_auth_delta_law(
        &[
            AuthEntry::Subscription(Some("work".to_owned())),
            AuthEntry::ApiKey(None),
        ],
        &[
            AuthEntry::ApiKey(None),
            AuthEntry::Subscription(Some("work".to_owned())),
        ],
    );
}

#[test]
fn test_auth_delta_law_holds_for_an_added_entry() {
    assert_auth_delta_law(&[AuthEntry::Subscription(Some("work".to_owned()))], &[
        AuthEntry::Subscription(Some("work".to_owned())),
        AuthEntry::ApiKey(None),
    ]);
}

#[test]
fn test_auth_delta_is_empty_for_an_unchanged_chain() {
    let prev = partial_with_auth(&[AuthEntry::ApiKey(None)]);

    let delta = prev.delta(partial_with_auth(&[AuthEntry::ApiKey(None)]));

    assert_eq!(delta.auth, None);
}

#[test]
fn test_assign_auth_chain_rejects_unrecognized_kind() {
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = r#"auth:=["bogus:x"]"#.parse().unwrap();

    assert!(partial.assign(kv).is_err());
}

/// A bare word is a name, so the chain accepts it and resolution decides.
/// Rejecting it here would need the credential store, which config never sees.
#[test]
fn test_assign_auth_chain_accepts_a_bare_name() {
    let mut partial = PartialAnthropicConfig::default();
    let kv: KvAssignment = r#"auth:=["personal"]"#.parse().unwrap();

    partial.assign(kv).unwrap();
    assert_eq!(
        partial.auth,
        Some(vec![AuthEntry::Named("personal".to_owned())])
    );
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
