use std::{
    collections::BTreeMap,
    error::Error as _,
    result,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use jp_config::{AppConfig, providers::llm::AuthEntry, types::api_key_env::ApiKeyEnv};
use jp_conversation::thread::Thread;
use jp_credentials::{
    CATEGORY_LLM, CredentialBackend, CredentialSecret, InMemoryCredentialBackend,
    PROVIDER_ANTHROPIC, StoreError, StoredCredential,
};
use jp_storage::resource_lock::InMemoryResourceLocker;

use super::*;
use crate::query::Truncation;

const SET_ENV_VAR: &str = if cfg!(windows) { "USERNAME" } else { "USER" };

/// A store that refuses writes once `read_only` is set.
#[derive(Debug, Default)]
struct Freezable {
    inner: InMemoryCredentialBackend,
    read_only: AtomicBool,
}

impl CredentialBackend for Freezable {
    fn describe(&self) -> String {
        "<freezable>".to_owned()
    }

    fn load(&self) -> result::Result<Option<String>, StoreError> {
        self.inner.load()
    }

    fn persist(&self, document: &str) -> result::Result<(), StoreError> {
        if self.read_only.load(Ordering::SeqCst) {
            return Err(StoreError::Rejected("store is read-only".into()));
        }
        self.inner.persist(document)
    }
}

fn provider(chain: &[&str]) -> (Anthropic, ModelDetails, CredentialStore) {
    provider_on(chain, Arc::new(InMemoryCredentialBackend::new()))
}

fn provider_on(
    chain: &[&str],
    backend: Arc<dyn CredentialBackend>,
) -> (Anthropic, ModelDetails, CredentialStore) {
    let mut config = AppConfig::new_test().providers.llm.anthropic;
    config.auth = chain.iter().map(|entry| entry.parse().unwrap()).collect();
    config.api_key_env = SET_ENV_VAR.into();
    let store = CredentialStore::new(backend, Arc::new(InMemoryResourceLocker::new()));
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

fn notices(events: &[result::Result<Event, StreamError>]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Ok(Event::Notice(notice)) => Some(notice.as_str()),
            _ => None,
        })
        .collect()
}

fn family_refused(family: &str) -> EventStream {
    Box::pin(stream::iter(vec![Err(
        StreamError::subscription_exhausted("refused", None, Some(family.into())),
    )]))
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
    // The rebuilt request resolves afresh and carries no switch notice, so
    // this is the only place the user learns where the request went.
    assert_eq!(notices(&events), vec![
        "skipping subscription:sub: already tried for this request",
        "subscription (sub) limit reached, continuing with subscription (sub2)",
    ]);
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
        notices(&events).last(),
        Some(&"subscription (sub2) limit reached, continuing with api key")
    );
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
async fn acp_stream_moves_only_the_exhausted_family_to_the_next_credential() {
    let (provider, model, store) = provider(&["sub:sub", "sub:sub2"]);
    let attempt = provider.resolve(&model.id.name).await.unwrap();
    let events = provider
        .with_acp_fallback(&model, attempt, family_refused("sonnet"))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        events.last().unwrap().as_ref().unwrap_err().kind,
        StreamErrorKind::CredentialChanged
    );

    for refused in ["claude-sonnet-5", "claude-sonnet-5-5"] {
        let next = provider.resolve(refused).await.unwrap();
        assert_eq!(
            next.selected.map(|selected| selected.entry),
            Some(AuthEntry::Subscription(Some("sub2".into()))),
            "{refused}"
        );
    }
    let other = provider.resolve("claude-opus-5").await.unwrap();
    assert_eq!(
        other.selected.map(|selected| selected.entry),
        Some(AuthEntry::Subscription(Some("sub".into())))
    );
    let snapshot = store.load().unwrap();
    let profiles = snapshot.profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC).unwrap();
    assert_eq!(
        profiles["sub"]
            .cooldowns
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["sonnet"]
    );
}

#[tokio::test]
async fn a_family_scoped_refusal_switches_a_model_with_a_display_name() {
    let (provider, mut model, _) = provider(&["sub:sub", "api_key"]);
    // The API's model listing sets `display_name`; credential routing must key
    // on `model.id.name` so family-scoped cooldowns still match.
    model.display_name = Some("Claude Sonnet 5".into());
    let attempt = provider.resolve(&model.id.name).await.unwrap();
    let events = provider
        .with_acp_fallback(&model, attempt, family_refused("sonnet"))
        .collect::<Vec<_>>()
        .await;
    assert_eq!(
        events.last().unwrap().as_ref().unwrap_err().kind,
        StreamErrorKind::CredentialChanged
    );

    // The rebuilt request goes to the API key, not back to the refused login.
    let query = ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
        truncation: Truncation::default(),
    };
    let context = QueryContext {
        root: "/work/project".into(),
        mcp_endpoint: None,
        invocation: None,
    };
    let stream = provider.start_query(&model, query, context).await.unwrap();
    assert!(matches!(stream.execution, ToolExecution::Caller));
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

/// The inherited login has no stored profile to cool down, so a rebuilt request
/// would resolve straight back to it; a credential change would loop.
#[tokio::test]
async fn acp_exhaustion_of_the_inherited_login_is_terminal() {
    let (provider, model, _) = provider(&["subscription", "api_key"]);
    let attempt = provider.resolve(model.name()).await.unwrap();
    let events = provider
        .with_acp_fallback(&model, attempt, exhausted())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 1);
    let error = events[0].as_ref().unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::SubscriptionExhausted);
    assert!(!error.is_retryable());
    assert_eq!(
        provider
            .resolve(model.name())
            .await
            .unwrap()
            .selected
            .map(|selected| selected.entry),
        Some(AuthEntry::Subscription(None))
    );
}

#[tokio::test]
async fn acp_exhaustion_is_terminal_when_the_cooldown_cannot_be_written() {
    let backend = Arc::new(Freezable::default());
    let (provider, model, store) = provider_on(&["sub:sub", "sub:sub2"], backend.clone());
    let attempt = provider.resolve(model.name()).await.unwrap();
    backend.read_only.store(true, Ordering::SeqCst);
    let events = provider
        .with_acp_fallback(&model, attempt, exhausted())
        .collect::<Vec<_>>()
        .await;
    assert_eq!(events.len(), 1);
    let error = events[0].as_ref().unwrap_err();
    assert_eq!(error.kind, StreamErrorKind::SubscriptionExhausted);
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

/// A mid-stream switch between API keys is invisible to the store, so a rebuilt
/// request would land on the spent key again.
#[tokio::test]
async fn rebuild_switch_between_api_keys_is_refused() {
    let (mut provider, model, _) = provider(&["api_key:a", "api_key:b"]);
    provider.config.api_key_env = ApiKeyEnv::Many(BTreeMap::from([
        ("a".into(), SET_ENV_VAR.into()),
        ("b".into(), SET_ENV_VAR.into()),
    ]));
    let attempt = provider.resolve(model.name()).await.unwrap();
    let error = StreamError::new(StreamErrorKind::InsufficientQuota, "out of credits");
    let next = provider
        .advance(&attempt, &error, model.name())
        .await
        .expect("the second key can serve this request");
    assert_eq!(
        next.selected.map(|selected| selected.entry),
        Some(AuthEntry::ApiKey(Some("b".into())))
    );
    assert!(
        provider
            .advance_for_rebuild(&attempt, &error, model.name())
            .await
            .is_none()
    );
}
