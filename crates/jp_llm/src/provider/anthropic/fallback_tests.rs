use std::{collections::BTreeMap, error::Error as _, sync::Arc};

use jp_config::{AppConfig, providers::llm::AuthEntry};
use jp_credentials::{
    CATEGORY_LLM, CredentialSecret, InMemoryCredentialBackend, PROVIDER_ANTHROPIC, StoredCredential,
};
use jp_storage::resource_lock::InMemoryResourceLocker;

use super::*;

fn provider(chain: &[&str]) -> (Anthropic, ModelDetails, CredentialStore) {
    let mut config = AppConfig::new_test().providers.llm.anthropic;
    config.auth = chain.iter().map(|entry| entry.parse().unwrap()).collect();
    config.api_key_env = if cfg!(windows) { "USERNAME" } else { "USER" }.into();
    let store = CredentialStore::new(
        Arc::new(InMemoryCredentialBackend::new()),
        Arc::new(InMemoryResourceLocker::new()),
    );
    store
        .mutate(|document| {
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub",
                registered(if cfg!(windows) {
                    "C:/accounts/sub"
                } else {
                    "/accounts/sub"
                }),
            );
            document.insert_profile(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                "sub2",
                registered(if cfg!(windows) {
                    "C:/accounts/sub2"
                } else {
                    "/accounts/sub2"
                }),
            );
            Ok(())
        })
        .unwrap();
    let mut provider = Anthropic::with_credential(&config, Credential::ApiKey("unused".into()));
    provider.fixed_credential = None;
    provider.store = Some(store.clone());
    (
        provider,
        ModelDetails::empty("anthropic/claude-sonnet-5".parse().unwrap()),
        store,
    )
}

fn registered(directory: &str) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::External {
            directory: directory.into(),
        },
        account_id: None,
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
        generation: 0,
    }
}

fn exhausted() -> EventStream {
    Box::pin(stream::iter(vec![Err(
        StreamError::subscription_exhausted("spent", None, Some("five_hour".into())),
    )]))
}

#[tokio::test]
async fn acp_stream_advances_before_requesting_a_host_rebuild() {
    let (provider, model, store) = provider(&["sub:sub", "sub:sub2", "api_key"]);
    let attempt = provider.resolve(model.name()).await.unwrap();
    let events = provider
        .with_acp_fallback(&model, attempt, exhausted())
        .collect::<Vec<_>>()
        .await;
    let error = events.last().unwrap().as_ref().unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::CredentialChanged);
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<StreamError>()
            .unwrap()
            .kind,
        StreamErrorKind::SubscriptionExhausted
    );
    let next = provider.resolve(model.name()).await.unwrap();
    assert_eq!(
        next.selected.as_ref().map(|selected| &selected.entry),
        Some(&AuthEntry::Subscription(Some("sub2".into())))
    );
    assert!(matches!(next.route, Route::Acp(Some(_))));
    let snapshot = store.load().unwrap();
    let profiles = snapshot.profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC).unwrap();
    assert_eq!(
        profiles["sub"]
            .cooldowns
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["five_hour"]
    );
    assert_eq!(profiles["sub2"].cooldowns, BTreeMap::new());

    let events = provider
        .with_acp_fallback(&model, next, exhausted())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        events.last().unwrap().as_ref().unwrap_err().kind,
        StreamErrorKind::CredentialChanged
    );
    let next = provider.resolve(model.name()).await.unwrap();
    assert_eq!(
        next.selected.map(|selected| selected.entry),
        Some(AuthEntry::ApiKey(None))
    );
    assert!(matches!(next.route, Route::Http(Credential::ApiKey(_))));
}

#[tokio::test]
async fn acp_stream_keeps_exhaustion_terminal_without_an_authorized_fallback() {
    let (provider, model, _) = provider(&["sub:sub"]);
    let attempt = provider.resolve(model.name()).await.unwrap();
    let events = provider
        .with_acp_fallback(&model, attempt, exhausted())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 1);
    let error = events[0].as_ref().unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::SubscriptionExhausted);
    assert_eq!(error.message(), "spent");
    assert!(!error.is_retryable());
}

#[tokio::test]
async fn acp_stream_does_not_switch_for_ordinary_rate_limits() {
    let (provider, model, store) = provider(&["sub:sub", "sub:sub2", "api_key"]);
    let attempt = provider.resolve(model.name()).await.unwrap();
    let input: EventStream = Box::pin(stream::iter(vec![Err(StreamError::rate_limit(None))]));
    let events = provider
        .with_acp_fallback(&model, attempt, input)
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].as_ref().unwrap_err().kind,
        StreamErrorKind::RateLimit
    );
    assert_eq!(
        provider
            .resolve(model.name())
            .await
            .unwrap()
            .selected
            .map(|selected| selected.entry),
        Some(AuthEntry::Subscription(Some("sub".into())))
    );
    assert_eq!(
        store
            .load()
            .unwrap()
            .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
            .unwrap()["sub"]
            .cooldowns,
        BTreeMap::new()
    );
}
