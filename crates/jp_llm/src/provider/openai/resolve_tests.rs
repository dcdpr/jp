use std::{collections::BTreeMap, sync::Arc};

use chrono::TimeZone as _;
use jp_credentials::{DEFAULT_COOLDOWN, InMemoryCredentialBackend, MAX_COOLDOWN};
use jp_storage::resource_lock::InMemoryResourceLocker;

use super::*;
use crate::StreamErrorKind;

/// A JWT whose payload nests a residency constraint of `eu`.
const TOKEN_EU: &str = "header.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjdC1uZXN0ZWQiLCJjaGF0Z3B0X2NvbXB1dGVfcmVzaWRlbmN5IjoiZXUifX0.sig";

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 9, 12, 0, 0).unwrap()
}

fn config(auth: Vec<AuthEntry>) -> OpenaiConfig {
    let mut config = jp_config::AppConfig::new_test().providers.llm.openai;
    config.auth = auth;
    config.api_key_env = "JP_TEST_OPENAI_KEY_UNSET".to_owned().into();
    config
}

fn store() -> CredentialStore {
    CredentialStore::new(
        Arc::new(InMemoryCredentialBackend::new()),
        Arc::new(InMemoryResourceLocker::default()),
    )
}

fn token_credential(token: &str) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::Token {
            token: token.to_owned(),
        },
        account_id: Some("acct-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    }
}

/// The profile's account-scoped cooldown, as stored.
fn cooldown(store: &CredentialStore, profile: &str) -> Option<DateTime<Utc>> {
    store
        .load()
        .unwrap()
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)?
        .get(profile)?
        .cooldowns
        .get(SCOPE_ACCOUNT)
        .copied()
}

fn insert(store: &CredentialStore, profile: &str, credential: &StoredCredential) {
    store
        .mutate(|document| {
            document.insert_profile(CATEGORY_LLM, PROVIDER_OPENAI, profile, credential.clone());
            Ok(())
        })
        .unwrap();
}

#[tokio::test]
async fn test_resolves_a_stored_token_profile() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));

    let attempt = resolve(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(
        attempt.credential,
        Credential::Bearer("bearer-1".to_owned())
    );
    assert_eq!(
        attempt.selected,
        Some(AuthEntry::Subscription(Some("personal".to_owned())))
    );
    assert!(attempt.is_subscription());
    assert_eq!(attempt.attribution.account_id.as_deref(), Some("acct-1"));
    assert!(attempt.notices.is_empty());
}

#[tokio::test]
async fn test_attribution_carries_the_tokens_residency() {
    let store = store();
    insert(&store, "eu", &token_credential(TOKEN_EU));

    let attempt = resolve(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(attempt.attribution.residency.as_deref(), Some("eu"));
}

#[tokio::test]
async fn test_skips_a_profile_needing_relogin_and_reports_why() {
    let store = store();
    let mut spent = token_credential("bearer-1");
    spent.needs_relogin = true;
    insert(&store, "spent", &spent);
    insert(&store, "fresh", &token_credential("bearer-2"));

    let attempt = resolve(
        &config(vec![
            AuthEntry::Subscription(Some("spent".to_owned())),
            AuthEntry::Subscription(Some("fresh".to_owned())),
        ]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(
        attempt.credential,
        Credential::Bearer("bearer-2".to_owned())
    );
    assert_eq!(attempt.notices, vec![
        "skipping profile:spent needs re-login; run `jp provider llm auth login openai --name \
         spent`"
            .to_owned()
    ]);
}

#[tokio::test]
async fn test_skips_a_cooling_down_profile() {
    let store = store();
    let mut cooling = token_credential("bearer-1");
    cooling
        .cooldowns
        .insert(SCOPE_ACCOUNT.to_owned(), now() + MAX_COOLDOWN);
    insert(&store, "cooling", &cooling);
    insert(&store, "fresh", &token_credential("bearer-2"));

    let attempt = resolve(
        &config(vec![
            AuthEntry::Subscription(Some("cooling".to_owned())),
            AuthEntry::Subscription(Some("fresh".to_owned())),
        ]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(
        attempt.credential,
        Credential::Bearer("bearer-2".to_owned())
    );
    assert_eq!(attempt.notices.len(), 1);
    assert!(attempt.notices[0].contains("cooling down"));
}

#[tokio::test]
async fn test_an_expired_cooldown_does_not_skip() {
    let store = store();
    let mut cooling = token_credential("bearer-1");
    cooling
        .cooldowns
        .insert(SCOPE_ACCOUNT.to_owned(), now() - MAX_COOLDOWN);
    insert(&store, "recovered", &cooling);

    let attempt = resolve(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(
        attempt.credential,
        Credential::Bearer("bearer-1".to_owned())
    );
}

#[tokio::test]
async fn test_a_model_scoped_cooldown_only_blocks_that_model() {
    let store = store();
    let mut cooling = token_credential("bearer-1");
    cooling
        .cooldowns
        .insert("gpt-5.6".to_owned(), now() + MAX_COOLDOWN);
    insert(&store, "personal", &cooling);
    let chain = config(vec![AuthEntry::Subscription(None)]);

    // The named model is cooling down.
    let blocked = resolve(&chain, Some(&store), "gpt-5.6", now()).await;
    assert!(blocked.is_err(), "expected the chain to be exhausted");

    // A different model on the same account still resolves.
    let allowed = resolve(&chain, Some(&store), "gpt-5.4-mini", now())
        .await
        .unwrap();
    assert_eq!(
        allowed.credential,
        Credential::Bearer("bearer-1".to_owned())
    );
}

#[tokio::test]
async fn test_unknown_named_profile_is_a_hard_error() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));

    let error = resolve(
        &config(vec![AuthEntry::Subscription(Some("work".to_owned()))]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ResolveError::UnknownProfile { ref name } if name == "work"),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("--name work"));
}

#[tokio::test]
async fn test_bare_profile_with_two_profiles_is_ambiguous() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));
    insert(&store, "work", &token_credential("bearer-2"));

    let error = resolve(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ResolveError::AmbiguousProfile { .. }));
    assert!(error.to_string().contains("personal, work"));
}

/// A bare name finds a stored subscription, without the chain having said which
/// kind it is.
#[tokio::test]
async fn test_a_bare_name_resolves_a_stored_subscription() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));

    let attempt = resolve(
        &config(vec![AuthEntry::Named("personal".to_owned())]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(
        attempt.selected,
        Some(AuthEntry::Subscription(Some("personal".to_owned())))
    );
    assert!(attempt.is_subscription());
}

/// A name nothing answers to reports what does exist, so the typo is visible.
#[tokio::test]
async fn test_an_unknown_bare_name_reports_the_stored_ones() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));

    let error = resolve(
        &config(vec![AuthEntry::Named("persnoal".to_owned())]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("persnoal"), "{error}");
    assert!(error.contains("personal"), "{error}");
}

#[tokio::test]
async fn test_bare_profile_with_no_profiles_names_the_login() {
    let error = resolve(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store()),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap_err();

    assert!(matches!(error, ResolveError::NoProfiles));
    assert!(
        error
            .to_string()
            .contains("jp provider llm auth login openai")
    );
}

#[tokio::test]
async fn test_a_missing_api_key_is_skipped_in_a_multi_entry_chain() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));

    let attempt = resolve(
        &config(vec![AuthEntry::ApiKey(None), AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(
        attempt.credential,
        Credential::Bearer("bearer-1".to_owned())
    );
    assert_eq!(attempt.notices, vec![
        "skipping api_key: environment variable JP_TEST_OPENAI_KEY_UNSET is not set".to_owned()
    ]);
}

#[tokio::test]
async fn test_a_missing_api_key_alone_is_the_pre_chain_failure() {
    // The single-entry default chain has to fail exactly as it did before
    // chains existed, so an unset key reads as a missing variable rather than
    // as an exhausted chain.
    let error = resolve(
        &config(vec![AuthEntry::ApiKey(None)]),
        None,
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ResolveError::MissingEnv(ref var) if var == "JP_TEST_OPENAI_KEY_UNSET"),
        "unexpected error: {error}"
    );
}

#[test]
fn test_preflight_accepts_a_chain_with_one_usable_entry() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));

    preflight(
        &config(vec![AuthEntry::ApiKey(None), AuthEntry::Subscription(None)]),
        Some(&store),
        "",
        now(),
    )
    .unwrap();
}

#[test]
fn test_preflight_rejects_a_chain_with_nothing_usable() {
    let store = store();
    let mut spent = token_credential("bearer-1");
    spent.needs_relogin = true;
    insert(&store, "spent", &spent);

    let error = preflight(
        &config(vec![AuthEntry::ApiKey(None), AuthEntry::Subscription(None)]),
        Some(&store),
        "",
        now(),
    )
    .unwrap_err();

    assert!(matches!(error, ResolveError::ChainExhausted { .. }));
}

#[test]
fn test_preflight_treats_an_expired_access_token_as_usable() {
    // Refreshing is a network call, which preflight must not make; resolution
    // does it instead, so an expired token cannot fail preflight.
    let store = store();
    insert(&store, "personal", &StoredCredential {
        secret: CredentialSecret::Oauth {
            access_token: "access-1".to_owned(),
            refresh_token: "refresh-1".to_owned(),
            expires_at: now() - MAX_COOLDOWN,
        },
        account_id: Some("acct-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    });

    preflight(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "",
        now(),
    )
    .unwrap();
}

#[tokio::test]
async fn test_advance_without_a_selected_entry_is_terminal() {
    // A directly injected credential has no chain position, so there is
    // nothing to advance past.
    let outcome = advance(
        &config(vec![AuthEntry::ApiKey(None)]),
        None,
        &AuthEntry::ApiKey(None),
        &StreamError::auth_rejected("refused"),
        "gpt-5.6",
        now(),
        false,
    )
    .await;

    assert!(outcome.is_none());
}

#[tokio::test]
async fn test_advance_records_relogin_and_moves_to_the_next_entry() {
    let store = store();
    insert(&store, "first", &token_credential("bearer-1"));
    insert(&store, "second", &token_credential("bearer-2"));
    let chain = config(vec![
        AuthEntry::Subscription(Some("first".to_owned())),
        AuthEntry::Subscription(Some("second".to_owned())),
    ]);

    let next = advance(
        &chain,
        Some(&store),
        &AuthEntry::Subscription(Some("first".to_owned())),
        &StreamError::auth_rejected("token revoked"),
        "gpt-5.6",
        now(),
        false,
    )
    .await
    .unwrap();

    assert_eq!(next.credential, Credential::Bearer("bearer-2".to_owned()));
    assert!(
        next.notices.iter().any(|notice| notice
            == "subscription (first) rejected, continuing with subscription (second)"),
        "unexpected notices: {:?}",
        next.notices
    );

    // The refusal is persisted, so a fresh invocation skips the profile too
    // rather than rediscovering it with another failed request.
    let document = store.load().unwrap();
    let stored = document
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)
        .unwrap()
        .get("first")
        .unwrap();
    assert!(stored.needs_relogin);
}

#[tokio::test]
async fn test_advance_records_a_cooldown_for_an_exhausted_profile() {
    let store = store();
    insert(&store, "first", &token_credential("bearer-1"));
    insert(&store, "second", &token_credential("bearer-2"));
    let chain = config(vec![
        AuthEntry::Subscription(Some("first".to_owned())),
        AuthEntry::Subscription(Some("second".to_owned())),
    ]);

    let next = advance(
        &chain,
        Some(&store),
        &AuthEntry::Subscription(Some("first".to_owned())),
        &StreamError::new(StreamErrorKind::SubscriptionExhausted, "limit reached"),
        "gpt-5.6",
        now(),
        false,
    )
    .await
    .unwrap();

    assert_eq!(next.credential, Credential::Bearer("bearer-2".to_owned()));

    let document = store.load().unwrap();
    let stored = document
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)
        .unwrap()
        .get("first")
        .unwrap();
    assert!(
        stored.active_cooldown("gpt-5.6", now()).is_some(),
        "expected a cooldown, got {:?}",
        stored.cooldowns
    );
    assert!(!stored.needs_relogin);
}

/// Without a redemption, the reported reset is the best evidence there is, so
/// the profile stays out until the window it names reopens.
#[tokio::test]
async fn test_advance_records_the_reported_reset_for_an_exhausted_window() {
    let store = store();
    insert(&store, "only", &token_credential("bearer-1"));

    let reset = now() + chrono::TimeDelta::hours(5);
    let mut error = StreamError::new(StreamErrorKind::SubscriptionExhausted, "limit reached");
    error.quota_reset = Some(reset);

    advance(
        &config(vec![AuthEntry::Subscription(Some("only".to_owned()))]),
        Some(&store),
        &AuthEntry::Subscription(Some("only".to_owned())),
        &error,
        "gpt-5.6",
        now(),
        false,
    )
    .await;

    assert_eq!(cooldown(&store, "only"), Some(reset));
}

/// The turn already spent a reset credit against this profile, so the usage
/// state these headers describe is one JP itself just changed.
/// Recording their reset timing would retire a profile the user can still reach
/// for the full length of the window they named — a week, at the cap.
#[tokio::test]
async fn test_advance_after_a_redemption_records_only_the_short_default() {
    let store = store();
    insert(&store, "only", &token_credential("bearer-1"));

    let mut error = StreamError::new(StreamErrorKind::SubscriptionExhausted, "limit reached");
    error.quota_reset = Some(now() + chrono::TimeDelta::days(30));

    advance(
        &config(vec![AuthEntry::Subscription(Some("only".to_owned()))]),
        Some(&store),
        &AuthEntry::Subscription(Some("only".to_owned())),
        &error,
        "gpt-5.6",
        now(),
        true,
    )
    .await;

    assert_eq!(cooldown(&store, "only"), Some(now() + DEFAULT_COOLDOWN));
}

/// A shortened cooldown must still take the spent profile out of the walk, or
/// the chain would hand the same refused credential back.
#[tokio::test]
async fn test_advance_after_a_redemption_still_reaches_the_next_entry() {
    let store = store();
    insert(&store, "first", &token_credential("bearer-1"));
    insert(&store, "second", &token_credential("bearer-2"));

    let mut error = StreamError::new(StreamErrorKind::SubscriptionExhausted, "limit reached");
    error.quota_reset = Some(now() + chrono::TimeDelta::days(30));

    let next = advance(
        &config(vec![
            AuthEntry::Subscription(Some("first".to_owned())),
            AuthEntry::Subscription(Some("second".to_owned())),
        ]),
        Some(&store),
        &AuthEntry::Subscription(Some("first".to_owned())),
        &error,
        "gpt-5.6",
        now(),
        true,
    )
    .await
    .unwrap();

    assert_eq!(next.credential, Credential::Bearer("bearer-2".to_owned()));
}

#[tokio::test]
async fn test_advance_records_nothing_for_a_malformed_request() {
    // A `400` is deterministic: every credential in the chain answers it the
    // same way. Recording a cooldown would take a working profile out of use
    // for half an hour over a request-shape bug.
    let store = store();
    insert(&store, "first", &token_credential("bearer-1"));
    insert(&store, "second", &token_credential("bearer-2"));

    advance(
        &config(vec![
            AuthEntry::Subscription(Some("first".to_owned())),
            AuthEntry::Subscription(Some("second".to_owned())),
        ]),
        Some(&store),
        &AuthEntry::Subscription(Some("first".to_owned())),
        &StreamError::other("System messages are not allowed (HTTP 400)"),
        "gpt-5.6",
        now(),
        false,
    )
    .await;

    let document = store.load().unwrap();
    let stored = document
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)
        .unwrap()
        .get("first")
        .unwrap();

    assert!(
        stored.cooldowns.is_empty(),
        "unexpected cooldowns: {:?}",
        stored.cooldowns
    );
    assert!(!stored.needs_relogin);
}

#[tokio::test]
async fn test_advance_records_nothing_for_a_context_window_overflow() {
    let store = store();
    insert(&store, "only", &token_credential("bearer-1"));

    advance(
        &config(vec![AuthEntry::Subscription(Some("only".to_owned()))]),
        Some(&store),
        &AuthEntry::Subscription(Some("only".to_owned())),
        &StreamError::context_window_exceeded("prompt too long"),
        "gpt-5.6",
        now(),
        false,
    )
    .await;

    let document = store.load().unwrap();
    let stored = document
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)
        .unwrap()
        .get("only")
        .unwrap();

    assert!(stored.cooldowns.is_empty());
    assert!(!stored.needs_relogin);
}

#[tokio::test]
async fn test_advance_is_terminal_when_the_chain_has_nothing_left() {
    let store = store();
    insert(&store, "only", &token_credential("bearer-1"));

    let outcome = advance(
        &config(vec![AuthEntry::Subscription(Some("only".to_owned()))]),
        Some(&store),
        &AuthEntry::Subscription(Some("only".to_owned())),
        &StreamError::auth_rejected("token revoked"),
        "gpt-5.6",
        now(),
        false,
    )
    .await;

    assert!(outcome.is_none());
}
