use super::*;
use crate::{AppConfig, assignment::KvAssignment};

#[test]
fn test_default_auth_chain_is_api_key() {
    let config = AppConfig::new_test();

    assert_eq!(config.providers.llm.openai.auth, vec![AuthEntry::ApiKey(
        None
    )]);
}

#[test]
fn test_default_codex_base_url() {
    let config = AppConfig::new_test();

    assert_eq!(
        config.providers.llm.openai.codex_base_url,
        "https://chatgpt.com/backend-api/codex"
    );
    assert_eq!(
        config.providers.llm.openai.codex_base_url_env,
        "JP_OPENAI_CODEX_BASE_URL"
    );
}

#[test]
fn test_validate_rejects_empty_auth_chain() {
    let mut config = AppConfig::new_test();
    config.providers.llm.openai.auth = vec![];

    let error = config.providers.llm.openai.validate().unwrap_err();

    assert!(
        error
            .to_string()
            .contains("providers.llm.openai.auth must contain at least one entry"),
        "unexpected error: {error}"
    );
}

#[test]
fn test_validate_rejects_duplicate_auth_entries() {
    let mut config = AppConfig::new_test();
    config.providers.llm.openai.auth = vec![
        AuthEntry::Subscription(Some("work".to_owned())),
        AuthEntry::ApiKey(None),
        AuthEntry::Subscription(Some("work".to_owned())),
    ];

    let error = config.providers.llm.openai.validate().unwrap_err();

    assert!(
        error
            .to_string()
            .contains("duplicate entry subscription:work"),
        "unexpected error: {error}"
    );
}

#[test]
fn test_assign_auth_chain() {
    let mut partial = PartialOpenaiConfig::default();
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
    let mut partial = PartialOpenaiConfig::default();
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
fn test_assign_codex_base_url() {
    let mut partial = PartialOpenaiConfig::default();
    let kv: KvAssignment = "codex_base_url=http://localhost:8080".parse().unwrap();

    partial.assign(kv).unwrap();

    assert_eq!(
        partial.codex_base_url.as_deref(),
        Some("http://localhost:8080")
    );
}
