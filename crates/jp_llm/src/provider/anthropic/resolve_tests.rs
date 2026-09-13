use std::{collections::BTreeMap, sync::Arc};

use assert_matches::assert_matches;
use datetime_literal::datetime;
use jp_credentials::InMemoryCredentialBackend;
use jp_storage::resource_lock::InMemoryResourceLocker;
use test_log::test;

use super::*;
use crate::StreamErrorKind;

const NOW: fn() -> DateTime<Utc> = || datetime!(2026-07-03 12:00:00 Z);

/// An environment variable that is always set, so `api_key` entries can resolve
/// without mutating the test process environment.
const SET_ENV_VAR: &str = if cfg!(windows) { "USERNAME" } else { "USER" };

/// An environment variable that is never set.
const UNSET_ENV_VAR: &str = "JP_TEST_RESOLVE_UNSET_VAR";

const MODEL: &str = "claude-opus-4-6";

fn anthropic_config(auth: &[&str], api_key_env: &str) -> AnthropicConfig {
    let mut config = jp_config::AppConfig::new_test().providers.llm.anthropic;
    config.auth = auth.iter().map(|s| s.parse().unwrap()).collect();
    config.api_key_env = api_key_env.to_owned().into();
    config
}

fn store_with(profiles: &[(&str, StoredCredential)]) -> StoreDocument {
    let mut document = StoreDocument::default();
    for (name, credential) in profiles {
        document.insert_profile(CATEGORY_LLM, PROVIDER_ANTHROPIC, name, credential.clone());
    }
    document
}

fn token_profile(token: &str) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::Token {
            token: token.to_owned(),
        },
        account_id: Some("uuid-1".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    }
}

fn oauth_profile(access_token: &str, expires_at: DateTime<Utc>) -> StoredCredential {
    StoredCredential {
        secret: CredentialSecret::Oauth {
            access_token: access_token.to_owned(),
            refresh_token: "rt".to_owned(),
            expires_at,
        },
        account_id: Some("uuid-2".to_owned()),
        email: None,
        cooldowns: BTreeMap::new(),
        needs_relogin: false,
    }
}

fn profile(name: &str) -> AuthEntry {
    AuthEntry::Subscription(Some(name.to_owned()))
}

/// The credential a walk landed on, failing the test if it needs a refresh.
fn ready(landing: Landing) -> Credential {
    match landing {
        Landing::Ready(credential) => credential,
        Landing::Stale { profile, .. } => {
            panic!("subscription:{profile} unexpectedly needs a refresh")
        }
    }
}

#[test]
fn test_named_profile_resolves_to_bearer() {
    let config = anthropic_config(&["subscription:personal"], UNSET_ENV_VAR);
    let store = store_with(&[("personal", token_profile("sk-ant-token"))]);

    let (landing, _selected, notices) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();

    assert_eq!(
        ready(landing),
        Credential::Bearer("sk-ant-token".to_owned())
    );
    assert!(notices.is_empty());
}

#[test]
fn test_bare_profile_resolves_sole_profile() {
    let config = anthropic_config(&["subscription"], UNSET_ENV_VAR);
    let store = store_with(&[("personal", token_profile("sk-ant-token"))]);

    let (landing, ..) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();
    assert_eq!(
        ready(landing),
        Credential::Bearer("sk-ant-token".to_owned())
    );
}

#[test]
fn test_bare_profile_with_zero_profiles_is_error() {
    let config = anthropic_config(&["subscription"], UNSET_ENV_VAR);
    let store = store_with(&[]);

    let error = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap_err();
    assert_matches!(error, ResolveError::NoProfiles);
}

#[test]
fn test_bare_profile_with_multiple_profiles_is_error() {
    let config = anthropic_config(&["subscription"], UNSET_ENV_VAR);
    let store = store_with(&[
        ("personal", token_profile("sk-a")),
        ("work", token_profile("sk-b")),
    ]);

    let error = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap_err();
    assert_matches!(
        error,
        ResolveError::AmbiguousProfile { names } if names == vec!["personal", "work"]
    );
}

#[test]
fn test_unknown_profile_is_error_even_with_later_entries() {
    // A chain entry naming a nonexistent profile is a config-shaped
    // mistake, not a skippable state: it fails even though `api_key`
    // could resolve.
    let config = anthropic_config(&["subscription:missing", "api_key"], SET_ENV_VAR);
    let store = store_with(&[("personal", token_profile("sk-a"))]);

    let error = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap_err();
    assert_matches!(error, ResolveError::UnknownProfile { name } if name == "missing");
}

#[test]
fn test_needs_relogin_profile_falls_through_with_notice() {
    let config = anthropic_config(&["subscription:personal", "api_key"], SET_ENV_VAR);
    let mut profile = token_profile("sk-a");
    profile.needs_relogin = true;
    let store = store_with(&[("personal", profile)]);

    let (landing, _selected, notices) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();

    assert_matches!(ready(landing), Credential::ApiKey(_));
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("needs re-login"), "{notices:?}");
}

#[test]
fn test_cooldown_scoped_to_model_family() {
    let config = anthropic_config(&["subscription:personal", "api_key"], SET_ENV_VAR);
    let mut profile = token_profile("sk-a");
    profile
        .cooldowns
        .insert("opus".to_owned(), datetime!(2026-07-03 13:00:00 Z));
    let store = store_with(&[("personal", profile)]);

    // An Opus request skips the cooling-down profile.
    let (landing, _selected, notices) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();
    assert_matches!(ready(landing), Credential::ApiKey(_));
    assert!(notices[0].contains("cooling down"), "{notices:?}");

    // A Haiku request on the same account resolves the profile.
    let (landing, _selected, notices) =
        walk_chain(&config, Some(&store), "claude-haiku-4-5", NOW()).unwrap();
    assert_eq!(ready(landing), Credential::Bearer("sk-a".to_owned()));
    assert!(notices.is_empty());
}

/// An expired access token is not a dead credential: resolution refreshes it in
/// place, so the walk reports it as stale rather than falling past it.
#[test]
fn test_expired_oauth_token_is_stale_rather_than_skipped() {
    let config = anthropic_config(&["subscription:personal", "api_key"], SET_ENV_VAR);
    let store = store_with(&[(
        "personal",
        oauth_profile("at", datetime!(2026-07-03 11:00:00 Z)),
    )]);

    let (landing, selected, notices) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();

    assert_matches!(landing, Landing::Stale { profile, refresh_token }
        if profile == "personal" && refresh_token == "rt");
    assert_eq!(selected, profile("personal"));
    assert!(notices.is_empty(), "{notices:?}");
}

/// A token inside the refresh window is refreshed early, so it cannot expire
/// between being resolved and being used.
#[test]
fn test_token_expiring_within_the_buffer_is_stale() {
    let config = anthropic_config(&["subscription:personal"], UNSET_ENV_VAR);
    let store = store_with(&[(
        "personal",
        // Two minutes out, inside the five-minute buffer.
        oauth_profile("at", datetime!(2026-07-03 12:02:00 Z)),
    )]);

    let (landing, ..) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();
    assert_matches!(landing, Landing::Stale { .. });
}

#[test]
fn test_live_oauth_token_resolves() {
    let config = anthropic_config(&["subscription:personal"], UNSET_ENV_VAR);
    let store = store_with(&[(
        "personal",
        oauth_profile("at", datetime!(2026-07-03 13:00:00 Z)),
    )]);

    let (landing, ..) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();
    assert_eq!(ready(landing), Credential::Bearer("at".to_owned()));
}

#[test]
fn test_default_chain_missing_env_reports_missing_env() {
    // Same failure shape as before credential chains existed.
    let config = anthropic_config(&["api_key"], UNSET_ENV_VAR);

    let error = walk_chain(&config, None, MODEL, NOW()).unwrap_err();
    assert_matches!(error, ResolveError::MissingEnv(var) if var == UNSET_ENV_VAR);
}

#[test]
fn test_multi_entry_chain_exhaustion_lists_reasons() {
    let config = anthropic_config(&["subscription:personal", "api_key"], UNSET_ENV_VAR);
    let mut profile = token_profile("sk-a");
    profile.needs_relogin = true;
    let store = store_with(&[("personal", profile)]);

    let error = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap_err();
    assert_matches!(error, ResolveError::ChainExhausted { reasons } if reasons.len() == 2);
}

#[test]
fn test_selected_entry_names_the_resolved_profile() {
    // A bare `profile` entry reports the profile it resolved to, so a log
    // line or a switch notice names a concrete credential rather than the
    // ambiguous chain entry the user wrote.
    let config = anthropic_config(&["subscription"], UNSET_ENV_VAR);
    let store = store_with(&[("personal", token_profile("sk-a"))]);

    let (_, selected, _) = walk_chain(&config, Some(&store), MODEL, NOW()).unwrap();
    assert_eq!(
        selected,
        AuthEntry::Subscription(Some("personal".to_owned()))
    );

    // An `api_key` entry reports itself.
    let config = anthropic_config(&["api_key"], SET_ENV_VAR);
    let (_, selected, _) = walk_chain(&config, None, MODEL, NOW()).unwrap();
    assert_eq!(selected, AuthEntry::ApiKey(None));
}

#[test]
fn test_switch_notice_names_both_credentials_and_the_reason() {
    let exhausted = StreamError::subscription_exhausted("spent", None, None);
    assert_eq!(
        switch_notice(&exhausted, &profile("personal"), Some(&profile("work"))),
        "subscription (personal) limit reached, continuing with subscription (work)"
    );

    // Falling through to the API key names it as written in the chain, so the
    // notice doubles as the audit trail for entering paid billing.
    assert_eq!(
        switch_notice(
            &exhausted,
            &profile("personal"),
            Some(&AuthEntry::ApiKey(None))
        ),
        "subscription (personal) limit reached, continuing with api key"
    );

    // A refused credential is a different reason: nothing was spent.
    let rejected = StreamError::auth_rejected("revoked");
    assert_eq!(
        switch_notice(
            &rejected,
            &profile("personal"),
            Some(&AuthEntry::ApiKey(None))
        ),
        "subscription (personal) rejected, continuing with api key"
    );

    // Billing exhaustion on a per-token account is neither of the above.
    let billing = StreamError::new(StreamErrorKind::InsufficientQuota, "no credit");
    assert_eq!(
        switch_notice(&billing, &AuthEntry::ApiKey(None), None),
        "api key quota exhausted, no more alternatives, aborting"
    );
}

/// Advancing away from `api_key` is impossible: it has no store entry to record
/// against, so re-resolution lands on the same entry and the failure is
/// terminal.
#[test(tokio::test)]
async fn test_advance_from_api_key_is_terminal() {
    let config = anthropic_config(&["api_key"], SET_ENV_VAR);
    let error = StreamError::new(StreamErrorKind::InsufficientQuota, "no credit");

    assert!(
        advance(
            &config,
            None,
            &AuthEntry::ApiKey(None),
            &error,
            MODEL,
            NOW()
        )
        .await
        .is_none()
    );
}

/// A store backed by memory, so a test can observe what resolution persisted
/// without touching the user's real credentials.
fn memory_store(profiles: &[(&str, StoredCredential)]) -> CredentialStore {
    let store = CredentialStore::new(
        Arc::new(InMemoryCredentialBackend::new()),
        Arc::new(InMemoryResourceLocker::new()),
    );

    store
        .mutate(|document| {
            for (name, credential) in profiles {
                document.insert_profile(CATEGORY_LLM, PROVIDER_ANTHROPIC, name, credential.clone());
            }
            Ok(())
        })
        .unwrap();

    store
}

/// Read a stored profile back out of the store.
fn stored(store: &CredentialStore, name: &str) -> StoredCredential {
    store
        .load()
        .unwrap()
        .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)
        .and_then(|profiles| profiles.get(name))
        .cloned()
        .expect("profile is stored")
}

/// Exhausting the active profile records a cooldown scoped to what the provider
/// reported, and lands on the next chain entry.
///
/// The recorded cooldown is the mechanism rather than a side effect: it is what
/// makes the *next* resolution skip the spent profile, so a mid-turn switch and
/// a fresh invocation share one code path.
#[test(tokio::test)]
async fn test_advance_records_scoped_cooldown_and_moves_to_next_profile() {
    let config = anthropic_config(
        &["subscription:personal", "subscription:work"],
        UNSET_ENV_VAR,
    );
    let store = memory_store(&[
        ("personal", token_profile("sk-personal")),
        ("work", token_profile("sk-work")),
    ]);

    // Four hours ahead of `NOW`: inside the seven-day cap, so the reported
    // reset is recorded verbatim rather than being capped or defaulted.
    let reset = datetime!(2026-07-03 16:00:00 Z);
    let error = StreamError::subscription_exhausted(
        "spent",
        Some(reset),
        Some("seven_day_opus".to_owned()),
    );

    let next = advance(
        &config,
        Some(&store),
        &profile("personal"),
        &error,
        MODEL,
        NOW(),
    )
    .await
    .expect("the chain has a second profile to fall to");

    assert_eq!(next.selected, Some(profile("work")));
    assert_eq!(next.credential, Credential::Bearer("sk-work".to_owned()));
    assert_eq!(
        next.notices.last().unwrap(),
        "subscription (personal) limit reached, continuing with subscription (work)"
    );

    // The cooldown is persisted against the spent profile, at the scope and
    // expiry the provider reported.
    let spent = stored(&store, "personal");
    assert_eq!(spent.cooldowns.get("seven_day_opus"), Some(&reset));
    assert!(!spent.needs_relogin);

    // A resolution after the switch skips the spent profile on its own,
    // without being told which entry was spent.
    let after = resolve(&config, Some(&store), MODEL, NOW()).await.unwrap();
    assert_eq!(after.selected, Some(profile("work")));

    // The cooldown is scoped to the Opus family, so a Haiku request on the
    // same account still resolves the profile that was spent for Opus.
    let haiku = resolve(&config, Some(&store), "claude-haiku-4-5", NOW())
        .await
        .unwrap();
    assert_eq!(haiku.selected, Some(profile("personal")));
}

/// A refused credential is marked for re-login rather than cooled down: no
/// allowance was spent, and waiting cannot help.
#[test(tokio::test)]
async fn test_advance_marks_refused_credential_for_relogin() {
    let config = anthropic_config(
        &["subscription:personal", "subscription:work"],
        UNSET_ENV_VAR,
    );
    let store = memory_store(&[
        ("personal", token_profile("sk-personal")),
        ("work", token_profile("sk-work")),
    ]);

    let next = advance(
        &config,
        Some(&store),
        &profile("personal"),
        &StreamError::auth_rejected("OAuth token has been revoked"),
        MODEL,
        NOW(),
    )
    .await
    .expect("the chain has a second profile to fall to");

    assert_eq!(next.selected, Some(profile("work")));
    assert_eq!(
        next.notices.last().unwrap(),
        "subscription (personal) rejected, continuing with subscription (work)"
    );

    let spent = stored(&store, "personal");
    assert!(spent.needs_relogin);
    assert!(spent.cooldowns.is_empty());
}

/// The last entry in the chain has nothing to fall to, so the failure stays
/// terminal.
/// The cooldown is still recorded, so the next invocation starts from an
/// accurate picture instead of rediscovering the exhaustion.
#[test(tokio::test)]
async fn test_advance_past_the_last_entry_is_terminal_but_still_records() {
    let config = anthropic_config(&["subscription:personal"], UNSET_ENV_VAR);
    let store = memory_store(&[("personal", token_profile("sk-personal"))]);

    let error = StreamError::subscription_exhausted("spent", None, None);
    assert!(
        advance(
            &config,
            Some(&store),
            &profile("personal"),
            &error,
            MODEL,
            NOW()
        )
        .await
        .is_none()
    );

    let spent = stored(&store, "personal");
    assert!(
        spent.cooldowns.contains_key(SCOPE_ACCOUNT),
        "a rejection without a reported scope cools down the whole account"
    );
}

/// Quota headers as they arrive on a response that *succeeded* while the
/// subscription window was already spent: paid extra usage served it.
fn spent_window_limits() -> UnifiedRateLimit {
    UnifiedRateLimit {
        status: Some("rejected".to_owned()),
        representative_claim: Some("seven_day".to_owned()),
        // 4 hours after `NOW`.
        reset: Some(1_783_094_400),
        overage_status: Some("allowed".to_owned()),
        ..UnifiedRateLimit::default()
    }
}

/// A successful response can report a spent allowance, which is the only
/// warning JP gets before extra usage starts billing.
///
/// Recording the cooldown is what makes the *next* request move off the profile
/// rather than silently spending money on it again.
#[test(tokio::test)]
async fn test_spent_window_on_a_successful_response_records_a_cooldown() {
    let store = memory_store(&[("personal", token_profile("sk-personal"))]);
    let watch = QuotaWatch::new(Some(&store), Some(&profile("personal")));

    let notices = watch.observe(&spent_window_limits(), NOW());

    assert_eq!(notices, vec![
        "weekly limit spent; this request was billed as extra usage".to_owned()
    ]);

    let spent = stored(&store, "personal");
    assert_eq!(
        spent.cooldowns.get("seven_day"),
        Some(&datetime!(2026-07-03 16:00:00 Z)),
        "the reported reset is recorded verbatim"
    );

    // The next resolution moves off the profile on its own.
    let config = anthropic_config(&["subscription:personal", "api_key"], SET_ENV_VAR);
    let after = resolve(&config, Some(&store), MODEL, NOW()).await.unwrap();
    assert_eq!(after.selected, Some(AuthEntry::ApiKey(None)));
}

/// An `api_key` request has no stored profile, so there is nothing to record
/// against; the notice still tells the user what happened.
#[test]
fn test_spent_window_without_a_profile_records_nothing() {
    let store = memory_store(&[("personal", token_profile("sk-personal"))]);
    let watch = QuotaWatch::new(Some(&store), Some(&AuthEntry::ApiKey(None)));

    let notices = watch.observe(&spent_window_limits(), NOW());
    assert_eq!(notices.len(), 1);

    assert!(
        stored(&store, "personal").cooldowns.is_empty(),
        "an api_key request must not record against an unrelated profile"
    );
}

/// A window that crossed a warning threshold is surfaced, and nothing is
/// recorded: the allowance is not spent yet.
#[test]
fn test_warning_threshold_is_surfaced_without_recording() {
    let store = memory_store(&[("personal", token_profile("sk-personal"))]);
    let watch = QuotaWatch::new(Some(&store), Some(&profile("personal")));

    let limits = UnifiedRateLimit {
        status: Some("allowed_warning".to_owned()),
        windows: vec![WindowUtilization {
            claim: "seven_day".to_owned(),
            utilization: Some(0.82),
            reset: Some(1_783_094_400),
            surpassed_threshold: Some(0.75),
        }],
        ..UnifiedRateLimit::default()
    };

    let notices = watch.observe(&limits, NOW());
    assert_eq!(notices, vec![
        "weekly limit 82% used, resets 2026-07-03 16:00 UTC".to_owned()
    ]);

    assert!(
        stored(&store, "personal").cooldowns.is_empty(),
        "a warning is not an exhausted window"
    );
}

/// A response with no quota headers at all (an API key account, or a provider
/// that stopped sending them) says nothing and changes nothing.
#[test]
fn test_response_without_quota_headers_is_silent() {
    let store = memory_store(&[("personal", token_profile("sk-personal"))]);
    let watch = QuotaWatch::new(Some(&store), Some(&profile("personal")));

    assert!(
        watch
            .observe(&UnifiedRateLimit::default(), NOW())
            .is_empty()
    );
    assert!(stored(&store, "personal").cooldowns.is_empty());
}
