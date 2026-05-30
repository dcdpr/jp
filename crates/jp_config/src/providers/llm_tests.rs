use test_log::test;

use super::*;
use crate::{
    assignment::KvAssignment,
    model::id::{PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
};

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
fn assign_alias_full_id_string() {
    let mut p = PartialLlmProviderConfig::default();

    let kv = KvAssignment::try_from_cli("aliases.opus", "anthropic/claude-opus-4").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.aliases.get("opus"),
        Some(&PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
            provider: Some(ProviderId::Anthropic),
            name: "claude-opus-4".parse().ok(),
        }))
    );
}

#[test]
fn assign_alias_pointing_to_another_alias() {
    let mut p = PartialLlmProviderConfig::default();

    let kv = KvAssignment::try_from_cli("aliases.coder", "opus").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p.aliases.get("coder"),
        Some(&PartialModelIdOrAliasConfig::Alias("opus".to_owned()))
    );
}

#[test]
fn assign_alias_nested_provider_and_name() {
    let mut p = PartialLlmProviderConfig::default();

    p.assign(KvAssignment::try_from_cli("aliases.haiku.provider", "anthropic").unwrap())
        .unwrap();
    p.assign(KvAssignment::try_from_cli("aliases.haiku.name", "claude-haiku-4-5").unwrap())
        .unwrap();
    assert_eq!(
        p.aliases.get("haiku"),
        Some(&PartialModelIdOrAliasConfig::Id(PartialModelIdConfig {
            provider: Some(ProviderId::Anthropic),
            name: "claude-haiku-4-5".parse().ok(),
        }))
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
