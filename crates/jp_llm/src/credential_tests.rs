use jp_config::model::id::ProviderId;
use test_log::test;

use super::*;

#[test]
fn test_provider_auth_dispatch() {
    // Anthropic is the only provider with stored-credential support.
    assert!(provider_auth(ProviderId::Anthropic).is_some());

    for id in [
        ProviderId::Cerebras,
        ProviderId::Google,
        ProviderId::Llamacpp,
        ProviderId::Ollama,
        ProviderId::Openai,
        ProviderId::Openrouter,
        ProviderId::Test,
    ] {
        assert!(provider_auth(id).is_none(), "provider: {id}");
    }
}

#[test]
fn test_credential_debug_redacts_secrets() {
    let api_key = Credential::ApiKey("sk-secret".to_owned());
    let bearer = Credential::Bearer("oauth-secret".to_owned());

    assert_eq!(format!("{api_key:?}"), "Credential::ApiKey(REDACTED)");
    assert_eq!(format!("{bearer:?}"), "Credential::Bearer(REDACTED)");
}
