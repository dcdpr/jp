//! Anthropic credential-chain resolution.
//!
//! Walks the `providers.llm.anthropic.auth` chain in order and produces the
//! first usable credential.
//! Config-shaped problems (an entry naming no stored profile, a malformed
//! store, a chain with no possibly-resolvable entry) are hard errors; per-entry
//! state (a profile needing re-login, an active cooldown, a missing environment
//! variable in a multi-entry chain) is a skip that falls through to the next
//! entry.
//!
//! Outcomes are recorded through the core-owned credential store
//! (`jp_credentials`): a spent profile gets a scoped cooldown, a refused one a
//! re-login marker, and the recorded state is what routes the next resolution
//! past it — a mid-turn switch and a fresh invocation share one code path.
//!
//! A request also remembers which entries it has already tried, which covers
//! what the store cannot: an `api_key` has no stored profile to mark, and a
//! best-effort write can fail.
//! Without it either would resolve again and strand the request on a credential
//! already known to be out.

use std::{
    collections::HashSet,
    mem,
    sync::{Arc, Mutex},
};

use async_anthropic::errors::{UnifiedRateLimit, WindowUtilization};
use chrono::{DateTime, Utc};
use jp_config::{
    providers::llm::anthropic::{AnthropicConfig, AuthEntry},
    types::api_key_env::ApiKeyEnv,
};
use jp_credentials::{
    CATEGORY_LLM, CredentialSecret, CredentialStore, PROVIDER_ANTHROPIC, SCOPE_ACCOUNT,
    StoreDocument, StoreError, StoreGuard, StoredCredential, UpdateOutcome, cooldown_until,
};
use tracing::{debug, warn};

use crate::{credential::Credential, error::StreamError, provider::anthropic::oauth};

/// Errors from walking the credential chain.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Store(#[from] StoreError),

    /// A `subscription:<name>` entry names a profile that is not stored.
    #[error(
        "providers.llm.anthropic.auth entry `subscription:{name}` matches no stored credential; \
         run `jp provider llm auth login anthropic --name {name}` to create it"
    )]
    UnknownProfile {
        /// The profile name the chain entry refers to.
        name: String,
    },

    /// A bare `subscription` entry with zero stored profiles.
    #[error(
        "providers.llm.anthropic.auth entry `subscription` matches no stored credential; run `jp \
         provider llm auth login anthropic` to create one"
    )]
    NoProfiles,

    /// A bare `subscription` entry with multiple stored profiles.
    #[error(
        "providers.llm.anthropic.auth entry `subscription` is ambiguous: several credentials are \
         stored ({}); name one with `subscription:<name>`",
        names.join(", ")
    )]
    AmbiguousProfile {
        /// Every stored profile name the bare entry could refer to.
        names: Vec<String>,
    },

    /// The single-entry `["api_key"]` chain with its environment variable
    /// unset.
    #[error("Missing environment variable: {0}")]
    MissingEnv(String),

    /// An `api_key` entry naming a key that `api_key_env` does not configure.
    #[error(transparent)]
    ApiKeyEnv(#[from] jp_config::types::api_key_env::ApiKeyEnvError),

    /// A bare name that both an API key and a subscription answer to.
    #[error(
        "`{name}` names both an API key and a subscription; write `api_key:{name}` or \
         `subscription:{name}`"
    )]
    AmbiguousName {
        /// The name written in the chain.
        name: String,
    },

    /// A bare name that nothing answers to.
    #[error(
        "no credential named `{name}`{}{}",
        if .keys.is_empty() { String::new() } else { format!(" (API keys: {})", .keys.join(", ")) },
        if .subscriptions.is_empty() {
            String::new()
        } else {
            format!(" (subscriptions: {})", .subscriptions.join(", "))
        }
    )]
    UnknownName {
        /// The name written in the chain.
        name: String,

        /// Every configured API key name.
        keys: Vec<String>,

        /// Every stored subscription name.
        subscriptions: Vec<String>,
    },

    /// Every chain entry was skipped.
    #[error(
        "no usable credential in the providers.llm.anthropic.auth chain: {}",
        reasons.join("; ")
    )]
    ChainExhausted {
        /// Why each chain entry was skipped, in chain order.
        reasons: Vec<String>,
    },

    /// An expired access token could not be refreshed.
    ///
    /// Distinct from a refused refresh, which retires the profile: this is the
    /// token endpoint being unreachable, which the retry layer above may
    /// outlast.
    #[error("could not refresh the access token for profile `{profile}`")]
    Refresh {
        profile: String,
        #[source]
        source: oauth::OauthError,
    },
}

/// What a walk of the chain landed on.
#[derive(Debug)]
enum Landing {
    /// A credential that can be sent as-is.
    Ready(Credential),

    /// An OAuth profile whose access token is spent or nearly so.
    ///
    /// Resolution refreshes it before use; preflight treats it as usable, since
    /// it has no business making a network call.
    Stale {
        profile: String,
        refresh_token: String,
    },
}

/// The chain entry a resolution landed on, and which stored credential backed
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Selected {
    /// The entry, with a bare `subscription` normalized to the profile it
    /// resolved to.
    pub entry: AuthEntry,

    /// The generation the stored profile held when this resolution read it.
    ///
    /// `None` for an entry with no stored profile behind it (`api_key`).
    /// Guarded writes carry it, so an outcome recorded for this credential
    /// cannot land on one that replaced it in the meantime.
    pub generation: Option<u64>,
}

/// A resolved chain attempt: the credential to send with, the entry that
/// produced it, and notices for entries skipped on the way there.
#[derive(Debug)]
pub(super) struct Attempt {
    /// The credential the request authenticates with.
    pub credential: Credential,

    /// Which chain entry produced the credential.
    ///
    /// `None` when the credential was injected directly instead of resolved
    /// from the chain, in which case there is no chain to advance.
    pub selected: Option<Selected>,

    /// User-facing notices for skipped entries, surfaced as chrome.
    pub notices: Vec<String>,

    /// The notice announcing that a failed request moved onto this credential.
    ///
    /// Kept apart from `notices` because a switch is a new event each time it
    /// happens, while a skip restates the same standing condition.
    pub switch: Option<String>,

    /// Chain entries this request has already tried and had fail.
    ///
    /// Resolution skips them.
    /// Without it, an entry whose failure leaves no trace in the store — an
    /// `api_key`, or a profile whose cooldown could not be written — resolves
    /// again on the next walk and strands the request on a credential already
    /// known to be out.
    tried: HashSet<AuthEntry>,
}

impl Attempt {
    /// An attempt over a credential handed in rather than resolved.
    ///
    /// There is no chain behind it, so there is nothing to advance to and no
    /// stored profile to record an outcome against.
    pub(super) fn injected(credential: Credential) -> Self {
        Self {
            credential,
            selected: None,
            notices: vec![],
            switch: None,
            tried: HashSet::new(),
        }
    }

    /// The notices to surface for this attempt, leaving none behind.
    ///
    /// A skip notice `seen` already holds is dropped; the switch notice is
    /// always kept.
    pub(super) fn take_notices(&mut self, seen: &SeenNotices) -> Vec<String> {
        let mut notices: Vec<String> = mem::take(&mut self.notices)
            .into_iter()
            .filter(|notice| seen.first(notice))
            .collect();

        notices.extend(self.switch.take());
        notices
    }
}

/// Notices one provider has already surfaced.
///
/// A notice that restates a standing condition (a credential skipped for the
/// same reason, a warning threshold already crossed) is shown once per provider
/// rather than on every request it sends.
/// Clones share one record.
#[derive(Debug, Clone, Default)]
pub(super) struct SeenNotices(Arc<Mutex<HashSet<String>>>);

impl SeenNotices {
    /// Record `key`, returning whether it had not been seen before.
    pub(super) fn first(&self, key: &str) -> bool {
        self.0.lock().expect("poisoned").insert(key.to_owned())
    }
}

/// Check that some entry of the chain could resolve, without any network use.
///
/// An access token that has expired counts as usable: [`resolve`] refreshes it
/// before the request, and refreshing is a network call preflight must not
/// make.
///
/// # Errors
///
/// Returns an error when the chain, the store, or one of their entries is
/// config-shaped-broken, or when no chain entry can resolve.
pub(super) fn preflight(
    config: &AnthropicConfig,
    store: Option<&CredentialStore>,
    model: &str,
    now: DateTime<Utc>,
) -> Result<(), ResolveError> {
    let snapshot = store.map(CredentialStore::load).transpose()?;
    walk_chain(config, snapshot.as_ref(), model, now, &HashSet::new()).map(drop)
}

/// Resolve the first usable credential in the chain.
///
/// An expired or nearly-expired OAuth access token is refreshed in place and
/// the rotated tokens persisted, so a long-lived profile keeps working without
/// the user logging in again.
///
/// The store is only read for profile entries, so the default `["api_key"]`
/// chain works without touching the store at all.
pub(super) async fn resolve(
    config: &AnthropicConfig,
    store: Option<&CredentialStore>,
    model: &str,
    now: DateTime<Utc>,
) -> Result<Attempt, ResolveError> {
    resolve_skipping(config, store, model, now, HashSet::new()).await
}

/// Resolve the chain, ignoring the entries in `tried`.
async fn resolve_skipping(
    config: &AnthropicConfig,
    store: Option<&CredentialStore>,
    model: &str,
    now: DateTime<Utc>,
    tried: HashSet<AuthEntry>,
) -> Result<Attempt, ResolveError> {
    // Each pass either resolves or retires one chain entry, so the walk
    // cannot cycle; the bound is belt against a profile that refuses to
    // settle.
    for _ in 0..=config.auth.len() {
        let snapshot = store.map(CredentialStore::load).transpose()?;
        let (landing, selected, notices) =
            walk_chain(config, snapshot.as_ref(), model, now, &tried)?;

        let (profile, refresh_token) = match landing {
            Landing::Ready(credential) => {
                debug!(
                    entry = %selected.entry,
                    mechanism = credential.kind(),
                    model,
                    skipped = notices.len(),
                    "Resolved provider credential."
                );

                return Ok(Attempt {
                    credential,
                    selected: Some(selected),
                    notices,
                    switch: None,
                    tried,
                });
            }
            Landing::Stale {
                profile,
                refresh_token,
            } => (profile, refresh_token),
        };

        // Whatever this returns, the next pass re-walks the chain and reads
        // the state it left: a rotated token resolves, a retired profile is
        // skipped with a notice the walk produces itself.
        refresh_under_lock(store, &profile, &refresh_token, now).await?;
    }

    Err(ResolveError::ChainExhausted {
        reasons: vec!["every profile needs a fresh login".to_owned()],
    })
}

/// Bring a stale profile's access token up to date, holding the store's
/// mutation lock across the exchange.
///
/// The lock is what makes the exchange safe to make at all.
/// Refresh tokens rotate, so two callers that read the same stale token and
/// both present it leave one holding a token the endpoint has already retired,
/// and an endpoint that treats reuse as a breach retires the whole grant.
/// Two JP requests are enough to reach that without any second terminal: a turn
/// and the title generation it spawns build their own providers and resolve
/// independently.
///
/// The document is re-read under the lock before anything is sent, so the
/// common case — another caller refreshed while this one waited — costs a
/// lock acquisition and no network call.
async fn refresh_under_lock(
    store: Option<&CredentialStore>,
    profile: &str,
    presented: &str,
    now: DateTime<Utc>,
) -> Result<(), ResolveError> {
    // A chain with no profile entries never lands on `Stale`, so there is no
    // store here to lock.
    let Some(store) = store else {
        return Ok(());
    };

    let guard = acquire(store).await?;
    let mut document = guard.load()?;

    let Some(refresh_token) = still_stale(&document, profile, presented, now) else {
        debug!(
            profile,
            "Access token was refreshed while this request waited for the lock."
        );
        return Ok(());
    };

    debug!(profile, "Access token expired; refreshing.");

    match oauth::refresh(&refresh_token).await {
        Ok(tokens) => rotate(&mut document, profile, &tokens),

        // The grant was refused: this profile cannot serve requests again
        // until the user logs in.
        Err(error) if error.is_rejection() => {
            warn!(%error, profile, "Refresh rejected; profile needs a fresh login.");
            retire(&mut document, profile);
        }

        // The endpoint could not be reached. The credential is probably fine,
        // and so is the next attempt; falling to another chain entry would not
        // help, since the request itself needs the same network.
        Err(source) => {
            return Err(ResolveError::Refresh {
                profile: profile.to_owned(),
                source,
            });
        }
    }

    guard.persist(&document)?;

    Ok(())
}

/// Take the store's mutation lock without parking an executor thread on it.
///
/// The lock is held across a network call, so a contender can wait as long as
/// the token endpoint takes to answer.
async fn acquire(store: &CredentialStore) -> Result<StoreGuard, ResolveError> {
    let store = store.clone();

    tokio::task::spawn_blocking(move || store.lock())
        .await
        .map_err(|error| {
            StoreError::Rejected(format!("credential store lock task failed: {error}"))
        })?
        .map_err(Into::into)
}

/// The refresh token to present, when `profile` still needs the refresh this
/// caller planned.
///
/// `None` once another caller has rotated the token or moved the expiry out,
/// which is the whole point of re-reading under the lock.
fn still_stale(
    document: &StoreDocument,
    profile: &str,
    presented: &str,
    now: DateTime<Utc>,
) -> Option<String> {
    match &document
        .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC)?
        .get(profile)?
        .secret
    {
        CredentialSecret::Oauth {
            refresh_token,
            expires_at,
            ..
        } if *expires_at <= now + oauth::EXPIRY_BUFFER && refresh_token == presented => {
            Some(refresh_token.clone())
        }
        _ => None,
    }
}

/// Write the tokens a refresh produced into the document.
///
/// The profile keeps its generation: rotating a token is the same credential
/// renewed, not a different one, and the outcome of a request already in flight
/// under it is still its own.
fn rotate(document: &mut StoreDocument, profile: &str, tokens: &oauth::Tokens) {
    let Some(credential) = document.profile_mut(CATEGORY_LLM, PROVIDER_ANTHROPIC, profile) else {
        return;
    };

    credential.secret = CredentialSecret::Oauth {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        expires_at: tokens.expires_at,
    };
    credential.needs_relogin = false;

    // A refresh that reports identity verifies a profile stored without one.
    if credential.account_id.is_none() {
        credential
            .account_id
            .clone_from(&tokens.identity.account_id);
        credential.email.clone_from(&tokens.identity.email);
    }

    debug!(profile, "Rotated stored OAuth tokens.");
}

/// Mark a profile as needing a fresh login after its refresh was refused.
fn retire(document: &mut StoreDocument, profile: &str) {
    if let Some(credential) = document.profile_mut(CATEGORY_LLM, PROVIDER_ANTHROPIC, profile) {
        credential.needs_relogin = true;
    }
}

/// Move to the next usable credential after `error`.
///
/// Records why `spent` is out (a scoped cooldown, or a re-login marker) and
/// re-resolves the chain; the recorded state is what makes resolution skip the
/// spent entry.
/// The returned attempt carries the switch notice, alongside any skip notices
/// resolution produced.
///
/// Returns `None` when the chain has nothing further to offer, which the caller
/// surfaces as the original, now-terminal error.
pub(super) async fn advance(
    config: &AnthropicConfig,
    store: Option<&CredentialStore>,
    spent: &Attempt,
    error: &StreamError,
    model: &str,
    now: DateTime<Utc>,
) -> Option<Attempt> {
    let selected = spent.selected.as_ref()?;

    record_outcome(store, selected, error, now);

    // Whether or not the store took the record, this entry is out for the rest
    // of the request. Carrying that in memory is what lets an `api_key` — which
    // has no stored profile to cool down — fall through to a profile behind it,
    // and what keeps a failed store write from stranding the request on a
    // credential already known to be spent.
    let mut tried = spent.tried.clone();
    tried.insert(selected.entry.clone());

    // Re-resolution reads the state just recorded against the same instant,
    // so a cooldown that starts now is already in effect for this walk.
    // An exhausted chain is not a new failure to report: the caller surfaces
    // the error that prompted the switch.
    let mut attempt = match resolve_skipping(config, store, model, now, tried).await {
        Ok(attempt) => attempt,
        Err(error) => {
            debug!(%error, "Credential chain exhausted after a failed request.");
            return None;
        }
    };

    attempt.switch = Some(switch_notice(
        error,
        &selected.entry,
        attempt.selected.as_ref().map(|next| &next.entry),
    ));

    Some(attempt)
}

/// Persist why the spent credential cannot serve the request.
///
/// Best-effort: a store that cannot be written costs this switch its
/// cross-invocation memory, not the request itself.
fn record_outcome(
    store: Option<&CredentialStore>,
    spent: &Selected,
    error: &StreamError,
    now: DateTime<Utc>,
) {
    // Only a stored profile has state to record against.
    let (AuthEntry::Subscription(Some(profile)), Some(generation)) =
        (&spent.entry, spent.generation)
    else {
        return;
    };
    let Some(store) = store else {
        return;
    };

    let result = if error.is_auth_rejected() {
        store.mark_needs_relogin(CATEGORY_LLM, PROVIDER_ANTHROPIC, profile, generation)
    } else {
        let scope = error.quota_scope.as_deref().unwrap_or(SCOPE_ACCOUNT);
        let until = cooldown_until(error.quota_reset, now);
        debug!(profile, scope, %until, "Recording quota cooldown.");
        store.record_cooldown(
            CATEGORY_LLM,
            PROVIDER_ANTHROPIC,
            profile,
            generation,
            scope,
            until,
        )
    };

    report_write(profile, result);
}

/// Log what a guarded write to a stored profile did.
///
/// Every outcome is survivable: the request has already failed and is moving
/// on, and the record only shapes what the next resolution sees.
fn report_write(profile: &str, result: Result<UpdateOutcome, StoreError>) {
    match result {
        Ok(UpdateOutcome::Updated) => {}
        Ok(UpdateOutcome::Missing) => warn!(
            profile,
            "Credential profile vanished before its state was recorded."
        ),
        Ok(UpdateOutcome::Superseded) => debug!(
            profile,
            "Profile was replaced by a login while this request ran; its outcome is not the new \
             credential's."
        ),
        Err(error) => warn!(%error, profile, "Could not record credential state."),
    }
}

/// Reads the subscription state a response reported.
///
/// The unified quota headers ride on every response, not only on a rejection,
/// so a spent allowance is visible without waiting for a request to fail.
/// That matters because an account with paid extra usage enabled keeps serving
/// requests past its window and bills them: without reading a successful
/// response, JP would never learn the allowance was gone.
#[derive(Debug, Clone, Default)]
pub(super) struct QuotaWatch {
    /// Where to record what a response reported.
    ///
    /// `None` for a request with no stored profile behind it (`api_key`), which
    /// has nothing to record against.
    store: Option<CredentialStore>,

    /// The stored profile the request authenticated as, and the generation it
    /// resolved at.
    profile: Option<(String, u64)>,

    /// Warnings already surfaced, so a threshold crossed once is not reported
    /// on every later response.
    seen: SeenNotices,
}

impl QuotaWatch {
    /// A watch over the profile `selected` names, if it names one.
    pub(super) fn new(store: Option<&CredentialStore>, selected: Option<&Selected>) -> Self {
        let profile = selected.and_then(|selected| match (&selected.entry, selected.generation) {
            (AuthEntry::Subscription(Some(name)), Some(generation)) => {
                Some((name.clone(), generation))
            }
            _ => None,
        });

        Self {
            store: profile.is_some().then(|| store.cloned()).flatten(),
            profile,
            seen: SeenNotices::default(),
        }
    }

    /// Share `seen` with the rest of the provider, so a warning surfaced by one
    /// request is not repeated by the next.
    #[must_use]
    pub(super) fn with_seen(mut self, seen: SeenNotices) -> Self {
        self.seen = seen;
        self
    }

    /// Act on what a response's quota headers reported.
    ///
    /// Returns the notices to surface as chrome.
    /// A spent allowance is recorded as a cooldown, so the next resolution
    /// moves off this profile instead of billing another request against paid
    /// extra usage.
    pub(super) fn observe(&self, limits: &UnifiedRateLimit, now: DateTime<Utc>) -> Vec<String> {
        if limits.is_empty() {
            return vec![];
        }

        // The window is spent even though the request went through, which
        // means paid extra usage served it.
        if limits.is_rejected() {
            let scope = limits
                .representative_claim
                .as_deref()
                .unwrap_or(SCOPE_ACCOUNT);

            self.record(scope, limits.reset.and_then(reset_at), now);

            return vec![format!(
                "{} spent; this request was billed as extra usage",
                window_name(scope)
            )];
        }

        // Keyed on the window and threshold rather than the text, which
        // changes with every percentage point the window fills.
        limits
            .warning()
            .filter(|window| {
                self.seen.first(&format!(
                    "{}@{:?}",
                    window.claim, window.surpassed_threshold
                ))
            })
            .map(|window| vec![warning_notice(window)])
            .unwrap_or_default()
    }

    /// Record a cooldown against the watched profile, best-effort.
    fn record(&self, scope: &str, reset: Option<DateTime<Utc>>, now: DateTime<Utc>) {
        let (Some(store), Some((profile, generation))) = (&self.store, &self.profile) else {
            return;
        };

        let until = cooldown_until(reset, now);
        debug!(
            profile,
            scope,
            %until,
            "Recording quota cooldown reported by a successful response."
        );

        report_write(
            profile,
            store.record_cooldown(
                CATEGORY_LLM,
                PROVIDER_ANTHROPIC,
                profile,
                *generation,
                scope,
                until,
            ),
        );
    }
}

/// Convert a reset reported as Unix seconds into a timestamp.
fn reset_at(seconds: u64) -> Option<DateTime<Utc>> {
    i64::try_from(seconds)
        .ok()
        .and_then(|secs| DateTime::from_timestamp(secs, 0))
}

/// How a usage window is named in user-facing output.
///
/// Matches the vocabulary Anthropic's own subscription UI uses, so the notice
/// names the same limit the user sees on their account.
fn window_name(claim: &str) -> &str {
    match claim {
        "five_hour" => "session limit",
        "seven_day" => "weekly limit",
        "seven_day_opus" => "Opus weekly limit",
        "seven_day_sonnet" => "Sonnet weekly limit",
        "overage" => "extra usage limit",
        other => other,
    }
}

/// The notice for a window that has crossed a warning threshold.
fn warning_notice(window: &WindowUtilization) -> String {
    let name = window_name(&window.claim);

    let used = window.utilization.map_or_else(
        || "nearly spent".to_owned(),
        |fraction| format!("{:.0}% used", fraction * 100.0),
    );

    match window.reset.and_then(reset_at) {
        Some(reset) => format!(
            "{name} {used}, resets {}",
            reset.format("%Y-%m-%d %H:%M UTC")
        ),
        None => format!("{name} {used}"),
    }
}

/// The one-line notice announcing a credential switch.
fn switch_notice(error: &StreamError, from: &AuthEntry, to: Option<&AuthEntry>) -> String {
    let reason = if error.is_auth_rejected() {
        "rejected"
    } else if error.kind == crate::StreamErrorKind::SubscriptionExhausted {
        "limit reached"
    } else {
        "quota exhausted"
    };

    match to {
        Some(to) => format!("{} {reason}, continuing with {}", label(from), label(to)),
        None => format!("{} {reason}, no more alternatives, aborting", label(from)),
    }
}

/// How a chain entry reads in a notice: its kind in words, plus the credential
/// it names.
fn label(entry: &AuthEntry) -> String {
    let kind = match entry {
        AuthEntry::ApiKey(_) => "api key",
        AuthEntry::Subscription(_) => "subscription",
        AuthEntry::Named(_) => "credential",
    };

    match entry.name() {
        Some(name) => format!("{kind} ({name})"),
        None => kind.to_owned(),
    }
}

/// Walk the chain and return the first usable credential, the entry that
/// produced it, and the notices for entries skipped along the way.
fn walk_chain(
    config: &AnthropicConfig,
    store: Option<&StoreDocument>,
    model: &str,
    now: DateTime<Utc>,
    tried: &HashSet<AuthEntry>,
) -> Result<(Landing, Selected, Vec<String>), ResolveError> {
    let mut notices = vec![];
    let mut reasons = vec![];

    for entry in &config.auth {
        // Settled here rather than in config, which sees neither source.
        let owned;
        let entry = match entry {
            AuthEntry::Named(name) => {
                owned = classify(&config.api_key_env, store, PROVIDER_ANTHROPIC, name)?;
                &owned
            }
            entry => entry,
        };

        match entry {
            AuthEntry::ApiKey(name) => {
                if tried.contains(entry) {
                    skip(
                        &mut notices,
                        &mut reasons,
                        format!("{entry}: already tried for this request"),
                    );
                    continue;
                }

                // A name no key answers to is a config mistake, not a
                // credential to fall past: the next entry would bill a
                // different key.
                let variable = config
                    .api_key_env
                    .variable(name.as_deref())
                    .map_err(ResolveError::ApiKeyEnv)?;

                if let Some(key) = super::super::api_key_chain::read_key(variable) {
                    return Ok((
                        Landing::Ready(Credential::ApiKey(key)),
                        Selected {
                            entry: entry.clone(),
                            generation: None,
                        },
                        notices,
                    ));
                }

                // Under the single-entry default chain, a missing key is
                // the same failure it was before chains existed.
                if config.auth.len() == 1 {
                    return Err(ResolveError::MissingEnv(variable.to_owned()));
                }

                skip(
                    &mut notices,
                    &mut reasons,
                    format!("{entry}: environment variable {variable} is not set"),
                );
            }

            AuthEntry::Subscription(name) => {
                match walk_profile(store, name.as_deref(), entry, model, now, tried)? {
                    ProfileStep::Landed(landing, selected) => {
                        return Ok((landing, selected, notices));
                    }
                    ProfileStep::Skip(reason) => skip(&mut notices, &mut reasons, reason),
                }
            }

            AuthEntry::Named(_) => unreachable!("classified above"),
        }
    }

    Err(ResolveError::ChainExhausted { reasons })
}

/// What one `subscription` entry of the chain offered.
enum ProfileStep {
    /// The entry produced a credential, or one a refresh can make usable.
    Landed(Landing, Selected),

    /// The entry cannot serve this request, for the reason given.
    Skip(String),
}

/// Evaluate one `subscription` chain entry.
fn walk_profile(
    store: Option<&StoreDocument>,
    name: Option<&str>,
    entry: &AuthEntry,
    model: &str,
    now: DateTime<Utc>,
    tried: &HashSet<AuthEntry>,
) -> Result<ProfileStep, ResolveError> {
    let (profile, stored) = lookup_profile(store, name)?;

    // A bare `subscription` entry is reported as the profile it resolved to,
    // so callers and logs name a concrete credential.
    let selected = Selected {
        entry: AuthEntry::Subscription(Some(profile.to_owned())),
        generation: Some(stored.generation),
    };

    // Both spellings are checked: the chain may hold the bare entry while the
    // tried set holds the profile it resolved to.
    if tried.contains(entry) || tried.contains(&selected.entry) {
        return Ok(ProfileStep::Skip(format!(
            "subscription:{profile}: already tried for this request"
        )));
    }

    if stored.needs_relogin {
        return Ok(ProfileStep::Skip(format!(
            "subscription:{profile} needs re-login; run `jp provider llm auth login anthropic \
             --name {profile}`"
        )));
    }

    if let Some((scope, until)) = stored.active_cooldown(model, now) {
        return Ok(ProfileStep::Skip(format!(
            "subscription:{profile} cooling down until {until} ({scope})"
        )));
    }

    let landing = match &stored.secret {
        CredentialSecret::Token { token } => Landing::Ready(Credential::Bearer(token.clone())),

        // Refreshed slightly before the deadline, so a token cannot expire
        // between being resolved and being used.
        CredentialSecret::Oauth {
            refresh_token,
            expires_at,
            ..
        } if *expires_at <= now + oauth::EXPIRY_BUFFER => Landing::Stale {
            profile: profile.to_owned(),
            refresh_token: refresh_token.clone(),
        },

        CredentialSecret::Oauth { access_token, .. } => {
            Landing::Ready(Credential::Bearer(access_token.clone()))
        }
    };

    Ok(ProfileStep::Landed(landing, selected))
}

/// Decide which kind of credential a bare name refers to.
///
/// A name both an API key and a subscription answer to is an error, not a
/// guess.
fn classify(
    api_key_env: &ApiKeyEnv,
    store: Option<&StoreDocument>,
    provider: &str,
    name: &str,
) -> Result<AuthEntry, ResolveError> {
    let keys = api_key_env.names();
    let subscriptions: Vec<&str> = store
        .and_then(|document| document.profiles(CATEGORY_LLM, provider))
        .map(|profiles| profiles.keys().map(String::as_str).collect())
        .unwrap_or_default();

    match (keys.contains(&name), subscriptions.contains(&name)) {
        (true, false) => Ok(AuthEntry::ApiKey(Some(name.to_owned()))),
        (false, true) => Ok(AuthEntry::Subscription(Some(name.to_owned()))),
        (true, true) => Err(ResolveError::AmbiguousName {
            name: name.to_owned(),
        }),
        (false, false) => Err(ResolveError::UnknownName {
            name: name.to_owned(),
            keys: keys.iter().map(|name| (*name).to_owned()).collect(),
            subscriptions: subscriptions
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        }),
    }
}

/// Record a skipped entry as both a chrome notice and an exhaustion reason.
fn skip(notices: &mut Vec<String>, reasons: &mut Vec<String>, reason: String) {
    debug!("Skipping credential chain entry: {reason}");
    notices.push(format!("skipping {reason}"));
    reasons.push(reason);
}

/// Look up the profile a chain entry refers to.
///
/// A named entry must exist; a bare `subscription` entry refers to the sole
/// stored profile and errors on zero or multiple candidates.
fn lookup_profile<'a>(
    store: Option<&'a StoreDocument>,
    name: Option<&str>,
) -> Result<(&'a str, &'a StoredCredential), ResolveError> {
    let profiles = store
        .expect("store is loaded when the chain contains profile entries")
        .profiles(CATEGORY_LLM, PROVIDER_ANTHROPIC);

    if let Some(name) = name {
        return profiles
            .and_then(|p| p.get_key_value(name))
            .map(|(profile, stored)| (profile.as_str(), stored))
            .ok_or_else(|| ResolveError::UnknownProfile {
                name: name.to_owned(),
            });
    }

    let mut iter = profiles.into_iter().flatten();
    let first = iter.next().ok_or(ResolveError::NoProfiles)?;

    if iter.next().is_some() {
        return Err(ResolveError::AmbiguousProfile {
            names: profiles
                .into_iter()
                .flatten()
                .map(|(name, _)| name.clone())
                .collect(),
        });
    }

    Ok((first.0.as_str(), first.1))
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
