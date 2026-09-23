use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use chrono::TimeZone as _;
use jp_credentials::{CredentialBackend, InMemoryCredentialBackend, MAX_COOLDOWN};
use jp_storage::resource_lock::InMemoryResourceLocker;

use super::*;
use crate::{StreamErrorKind, credential::AccountIdentity};

/// An environment variable that is always set, so `api_key` entries resolve
/// without mutating the test process environment.
const SET_ENV_VAR: &str = if cfg!(windows) { "USERNAME" } else { "USER" };

/// A second always-set variable, for a chain that needs two keys.
const ALSO_SET_ENV_VAR: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

/// An attempt that landed on `entry`, as `advance` receives it.
fn attempt_on(entry: AuthEntry, generation: Option<u64>) -> Attempt {
    Attempt {
        credential: Credential::Bearer("spent".to_owned()),
        attribution: Attribution::default(),
        selected: Some(entry),
        generation,
        notices: vec![],
        tried: HashSet::new(),
    }
}

/// A stored OAuth profile whose access token has expired.
fn stale_credential() -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::Oauth {
            access_token: "at-old".to_owned(),
            refresh_token: "rt-old".to_owned(),
            expires_at: now() - MAX_COOLDOWN,
        },
        account_id: Some("acct-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
        generation: 0,
    }
}

/// A refresh that counts its calls and answers with `outcome`.
///
/// It sleeps before answering, so a second caller has the chance to read the
/// same stale token while the first exchange is in flight.
fn counting_refresh(
    calls: Arc<AtomicUsize>,
    outcome: fn() -> Result<oauth::Tokens, oauth::OauthError>,
) -> impl Fn(String) -> BoxFuture<'static, Result<oauth::Tokens, oauth::OauthError>> + Send + Sync {
    move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            outcome()
        })
    }
}

/// A refresh that succeeds with a rotated pair.
#[expect(
    clippy::unnecessary_wraps,
    reason = "passed where `counting_refresh` takes a refresh outcome"
)]
fn fresh_tokens() -> Result<oauth::Tokens, oauth::OauthError> {
    Ok(oauth::Tokens {
        access_token: "at-new".to_owned(),
        refresh_token: "rt-new".to_owned(),
        expires_at: now() + MAX_COOLDOWN,
        identity: AccountIdentity::default(),
    })
}

/// A backend whose writes start failing once `fail` is set.
#[derive(Debug, Default)]
struct FlakyBackend {
    inner: InMemoryCredentialBackend,
    fail: AtomicBool,
}

impl CredentialBackend for FlakyBackend {
    fn describe(&self) -> String {
        "<flaky>".to_owned()
    }

    fn load(&self) -> Result<Option<String>, StoreError> {
        self.inner.load()
    }

    fn persist(&self, document: &str) -> Result<(), StoreError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(StoreError::Rejected("disk full".to_owned()));
        }

        self.inner.persist(document)
    }
}

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
        generation: 0,
    }
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
        "skipping subscription:spent needs re-login; run `jp provider llm auth login openai \
         --name spent`"
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
    let allowed = resolve(&chain, Some(&store), "gpt-5.5", now())
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
        generation: 0,
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
        &Attempt::injected(
            Credential::ApiKey("sk-1".to_owned()),
            Attribution::default(),
        ),
        &StreamError::auth_rejected("refused"),
        "gpt-5.6",
        now(),
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
        &attempt_on(AuthEntry::Subscription(Some("first".to_owned())), Some(0)),
        &StreamError::auth_rejected("token revoked"),
        "gpt-5.6",
        now(),
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
        &attempt_on(AuthEntry::Subscription(Some("first".to_owned())), Some(0)),
        &StreamError::new(StreamErrorKind::SubscriptionExhausted, "limit reached"),
        "gpt-5.6",
        now(),
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
        &attempt_on(AuthEntry::Subscription(Some("first".to_owned())), Some(0)),
        &StreamError::other("System messages are not allowed (HTTP 400)"),
        "gpt-5.6",
        now(),
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
        &attempt_on(AuthEntry::Subscription(Some("only".to_owned())), Some(0)),
        &StreamError::context_window_exceeded("prompt too long"),
        "gpt-5.6",
        now(),
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
        &attempt_on(AuthEntry::Subscription(Some("only".to_owned())), Some(0)),
        &StreamError::auth_rejected("token revoked"),
        "gpt-5.6",
        now(),
    )
    .await;

    assert!(outcome.is_none());
}

/// Two resolutions of the same stale profile exchange its refresh token once.
///
/// Refresh tokens rotate, so a second exchange would present a token the first
/// one already consumed.
#[tokio::test]
async fn test_concurrent_resolutions_refresh_once() {
    let store = store();
    insert(&store, "personal", &stale_credential());
    let chain = config(vec![AuthEntry::Subscription(None)]);
    let calls = Arc::new(AtomicUsize::new(0));
    let refresh = counting_refresh(calls.clone(), fresh_tokens);

    let (first, second) = tokio::join!(
        resolve_skipping(
            &chain,
            Some(&store),
            "gpt-5.6",
            now(),
            HashSet::new(),
            &refresh
        ),
        resolve_skipping(
            &chain,
            Some(&store),
            "gpt-5.6",
            now(),
            HashSet::new(),
            &refresh
        ),
    );

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        first.unwrap().credential,
        Credential::Bearer("at-new".to_owned())
    );
    assert_eq!(
        second.unwrap().credential,
        Credential::Bearer("at-new".to_owned())
    );
}

/// Rotated tokens that cannot be stored stop resolution.
///
/// The refresh token presented is spent; resolving again would present it a
/// second time.
#[tokio::test]
async fn test_a_failed_rotation_write_stops_resolution() {
    let backend = Arc::new(FlakyBackend::default());
    let store = CredentialStore::new(backend.clone(), Arc::new(InMemoryResourceLocker::default()));
    insert(&store, "personal", &stale_credential());
    backend.fail.store(true, Ordering::SeqCst);

    let calls = Arc::new(AtomicUsize::new(0));
    let refresh = counting_refresh(calls.clone(), fresh_tokens);

    let error = resolve_skipping(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
        HashSet::new(),
        &refresh,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ResolveError::Store(_)),
        "unexpected error: {error}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// A refused grant retires the profile, so later invocations skip it.
#[tokio::test]
async fn test_a_refused_refresh_retires_the_profile() {
    let store = store();
    insert(&store, "personal", &stale_credential());
    let refresh = counting_refresh(Arc::default(), || {
        Err(oauth::OauthError::Status {
            status: 400,
            body: "invalid_grant".to_owned(),
        })
    });

    let error = resolve_skipping(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
        HashSet::new(),
        &refresh,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ResolveError::ChainExhausted { .. }),
        "unexpected error: {error}"
    );
    assert!(stored(&store, "personal").needs_relogin);
}

/// A throttled token endpoint says nothing about the credential, so the profile
/// stays usable for the next invocation.
#[tokio::test]
async fn test_a_throttled_refresh_keeps_the_profile() {
    let store = store();
    insert(&store, "personal", &stale_credential());
    let refresh = counting_refresh(Arc::default(), || {
        Err(oauth::OauthError::Status {
            status: 429,
            body: "slow down".to_owned(),
        })
    });

    let error = resolve_skipping(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-5.6",
        now(),
        HashSet::new(),
        &refresh,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ResolveError::Refresh { .. }),
        "unexpected error: {error}"
    );
    assert!(!stored(&store, "personal").needs_relogin);
}

/// A refused API key leaves nothing in the store, so only the request's own
/// record of what it tried lets the chain reach the subscription behind it.
#[tokio::test]
async fn test_a_refused_api_key_falls_through_to_a_subscription() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));
    let mut chain = config(vec![AuthEntry::ApiKey(None), AuthEntry::Subscription(None)]);
    chain.api_key_env = SET_ENV_VAR.into();

    let first = resolve(&chain, Some(&store), "gpt-5.6", now())
        .await
        .unwrap();
    assert_eq!(first.selected, Some(AuthEntry::ApiKey(None)));

    let next = advance(
        &chain,
        Some(&store),
        &first,
        &StreamError::new(StreamErrorKind::InsufficientQuota, "no credit"),
        "gpt-5.6",
        now(),
    )
    .await
    .unwrap();

    assert_eq!(
        next.selected,
        Some(AuthEntry::Subscription(Some("personal".to_owned())))
    );
    assert!(
        next.notices
            .iter()
            .any(|notice| notice
                == "api key quota exhausted, continuing with subscription (personal)"),
        "unexpected notices: {:?}",
        next.notices
    );
}

/// Two named keys: the first one refused moves the request onto the second, and
/// the second refused ends it rather than retrying the first.
#[tokio::test]
async fn test_an_entry_is_tried_once_per_request() {
    let mut chain = config(vec![
        AuthEntry::ApiKey(Some("work".to_owned())),
        AuthEntry::ApiKey(Some("personal".to_owned())),
    ]);
    chain.api_key_env = ApiKeyEnv::Many(BTreeMap::from([
        ("work".to_owned(), SET_ENV_VAR.to_owned()),
        ("personal".to_owned(), ALSO_SET_ENV_VAR.to_owned()),
    ]));
    let refused = StreamError::auth_rejected("invalid key");

    let first = resolve(&chain, None, "gpt-5.6", now()).await.unwrap();
    assert_eq!(
        first.selected,
        Some(AuthEntry::ApiKey(Some("work".to_owned())))
    );

    let second = advance(&chain, None, &first, &refused, "gpt-5.6", now())
        .await
        .unwrap();
    assert_eq!(
        second.selected,
        Some(AuthEntry::ApiKey(Some("personal".to_owned())))
    );

    let third = advance(&chain, None, &second, &refused, "gpt-5.6", now()).await;
    assert!(third.is_none(), "the chain offered a key it already tried");
}

/// A model only the API serves is reached through the key behind a
/// subscription, rather than sent to a host that answers it with a `404`.
#[tokio::test]
async fn test_an_api_only_model_skips_the_subscription() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));
    let mut chain = config(vec![AuthEntry::Subscription(None), AuthEntry::ApiKey(None)]);
    chain.api_key_env = SET_ENV_VAR.into();

    let attempt = resolve(&chain, Some(&store), "gpt-4.1", now())
        .await
        .unwrap();

    assert_eq!(attempt.selected, Some(AuthEntry::ApiKey(None)));
    assert_eq!(attempt.notices, vec![
        "skipping subscription:personal does not serve gpt-4.1; only an `api_key` entry reaches it"
            .to_owned()
    ]);

    // A model the plan does serve still lands on the subscription.
    let attempt = resolve(&chain, Some(&store), "gpt-5.6-luna", now())
        .await
        .unwrap();
    assert!(attempt.is_subscription());
}

/// With no key behind it, the chain says why the subscription could not serve
/// the model, and records nothing against a credential that did no wrong.
#[tokio::test]
async fn test_an_api_only_model_on_a_subscription_only_chain_names_the_fix() {
    let store = store();
    insert(&store, "personal", &token_credential("bearer-1"));

    let error = resolve(
        &config(vec![AuthEntry::Subscription(None)]),
        Some(&store),
        "gpt-4.1",
        now(),
    )
    .await
    .unwrap_err();

    assert!(
        matches!(error, ResolveError::ChainExhausted { .. }),
        "unexpected error: {error}"
    );
    assert!(error.to_string().contains("`api_key`"), "{error}");

    let stored = stored(&store, "personal");
    assert!(stored.cooldowns.is_empty());
    assert!(!stored.needs_relogin);
}

/// Every chain entry an error tells the user to write has to parse, or
/// following the advice produces another configuration error.
#[test]
fn test_every_suggested_entry_parses() {
    for error in [
        ResolveError::UnknownProfile {
            name: "work".to_owned(),
        },
        ResolveError::NoProfiles,
        ResolveError::AmbiguousProfile {
            names: vec!["personal".to_owned(), "work".to_owned()],
        },
    ] {
        let message = error.to_string();
        let suggested: Vec<_> = message
            .split('`')
            .skip(1)
            .step_by(2)
            .filter(|quoted| quoted.starts_with("subscription"))
            .map(|quoted| quoted.replace("<name>", "work"))
            .collect();

        assert!(!suggested.is_empty(), "{message}");
        for entry in suggested {
            assert!(
                matches!(entry.parse::<AuthEntry>(), Ok(AuthEntry::Subscription(_))),
                "{entry} in: {message}"
            );
        }
    }
}

fn stored(store: &CredentialStore, profile: &str) -> StoredCredential {
    store
        .load()
        .unwrap()
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)
        .unwrap()
        .get(profile)
        .unwrap()
        .clone()
}
