use test_log::test;

use super::*;
use crate::assignment::KvAssignment;

#[test]
fn test_provider_config_anthropic() {
    let mut p = PartialLlmProviderConfig::default();

    let kv = KvAssignment::try_from_cli("anthropic.api_key_env", "MY_ANTHROPIC_KEY").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.anthropic.api_key_env.as_deref(), Some("MY_ANTHROPIC_KEY"));
}

#[test]
fn test_provider_config_openai() {
    let mut p = PartialLlmProviderConfig::default();

    let kv = KvAssignment::try_from_cli("openai.base_url", "https://custom.openai.com").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.openai.base_url.as_deref(),
        Some("https://custom.openai.com")
    );
}

#[test]
fn test_provider_config_openrouter_referrer() {
    let mut p = PartialLlmProviderConfig::default();

    let kv = KvAssignment::try_from_cli("openrouter.app_referrer", "").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.openrouter.app_referrer, Some(String::new()));

    let kv = KvAssignment::try_from_cli("openrouter.app_referrer", "https://example.com").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.openrouter.app_referrer,
        Some("https://example.com".to_string())
    );
}
