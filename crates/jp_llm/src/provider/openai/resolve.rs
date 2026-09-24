//! `OpenAI` credential-chain resolution.
//!
//! Walks the `providers.llm.openai.auth` chain in order and produces the first
//! usable credential.
//! Config-shaped problems (an entry naming no stored profile, a malformed
//! store, a chain with no possibly-resolvable entry) are hard errors; per-entry
//! state (a profile needing re-login, an active cooldown, a missing environment
//! variable in a multi-entry chain) is a skip that falls through to the next
//! entry.
//!
//! Outcomes are recorded through the core-owned credential store
//! (`jp_credentials`): a refused profile gets a re-login marker, a spent one a
//! cooldown, and the recorded state is what routes the next resolution past it.
//! A mid-turn switch and a fresh invocation share that one code path.
//!
//! A request also remembers which entries it has already tried, which covers
//! what the store cannot: an `api_key` has no stored profile to mark, and a
//! best-effort write can fail.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use jp_config::{
    providers::llm::{AuthEntry, openai::OpenaiConfig},
    types::api_key_env::ApiKeyEnv,
};
use jp_credentials::{
    CATEGORY_LLM, CredentialSecret, CredentialStore, PROVIDER_OPENAI, SCOPE_ACCOUNT, StoreDocument,
    StoreError, StoreGuard, StoredCredential, UpdateOutcome, cooldown_until,
};
use tracing::{debug, warn};

use crate::{
    credential::Credential,
    error::{StreamError, StreamErrorKind},
    provider::openai::oauth,
};

/// Errors from walking the credential chain.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Store(#[from] StoreError),

    /// A `subscription:<name>` entry names a profile that is not stored.
    #[error(
        "providers.llm.openai.auth entry `subscription:{name}` matches no stored credential; run \
         `jp provider llm auth login openai --name {name}` to create it"
    )]
    UnknownProfile {
        /// The profile name the chain entry refers to.
        name: String,
    },

    /// A bare `subscription` entry with zero stored profiles.
    #[error(
        "providers.llm.openai.auth entry `subscription` matches no stored credential; run `jp \
         provider llm auth login openai` to create one"
    )]
    NoProfiles,

    /// A bare `subscription` entry with multiple stored profiles.
    #[error(
        "providers.llm.openai.auth entry `subscription` is ambiguous: several credentials are \
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
        "no usable credential in the providers.llm.openai.auth chain: {}",
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
    Ready(Credential, Attribution),

    /// An OAuth profile whose access token is spent or nearly so.
    ///
    /// Resolution refreshes it before use; preflight treats it as usable, since
    /// it has no business making a network call.
    Stale {
        profile: String,
        refresh_token: String,
    },
}

/// Trades a refresh token for fresh tokens.
///
/// A parameter rather than a direct call to [`oauth::refresh`], so a test can
/// count exchanges without reaching the network.
type Refresh =
    dyn Fn(String) -> BoxFuture<'static, Result<oauth::Tokens, oauth::OauthError>> + Send + Sync;

/// The production [`Refresh`]: the `OpenAI` token endpoint.
fn network_refresh(
    refresh_token: String,
) -> BoxFuture<'static, Result<oauth::Tokens, oauth::OauthError>> {
    Box::pin(async move { oauth::refresh(&refresh_token).await })
}

/// Account attribution a subscription request has to carry.
///
/// Empty for an API key, which is attributed by the key itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Attribution {
    /// The `ChatGPT` account the credential belongs to, sent as
    /// `ChatGPT-Account-Id`.
    pub account_id: Option<String>,

    /// The compute-residency constraint the token carries, sent as
    /// `x-openai-internal-codex-residency`.
    pub residency: Option<String>,
}

/// A resolved chain attempt: the credential to send with, the entry that
/// produced it, and notices for entries skipped on the way there.
#[derive(Debug)]
pub(super) struct Attempt {
    /// The credential the request authenticates with.
    pub credential: Credential,

    /// Account attribution the subscription endpoint requires.
    pub attribution: Attribution,

    /// Which chain entry produced the credential, with a bare `subscription`
    /// normalized to the profile it resolved to.
    ///
    /// `None` when the credential was injected directly instead of resolved
    /// from the chain, in which case there is no chain to advance.
    pub selected: Option<AuthEntry>,

    /// The generation the stored profile held when this resolution read it.
    ///
    /// `None` for an entry with no stored profile behind it (`api_key`), or an
    /// injected credential.
    /// Guarded writes carry it, so an outcome recorded for this credential
    /// cannot land on one that replaced it in the meantime.
    pub generation: Option<u64>,

    /// User-facing notices for skipped entries, surfaced as chrome.
    pub notices: Vec<String>,

    /// Chain entries this request has already tried and had fail.
    ///
    /// Resolution skips them.
    /// Without it, an entry whose failure leaves no trace in the store (an
    /// `api_key`, or a profile whose cooldown could not be written) resolves
    /// again on the next walk and strands the request on a credential already
    /// known to be out.
    tried: HashSet<AuthEntry>,
}

impl Attempt {
    /// An attempt over a credential handed in rather than resolved.
    ///
    /// There is no chain behind it, so there is nothing to advance to and no
    /// stored profile to record an outcome against.
    pub(super) fn injected(credential: Credential, attribution: Attribution) -> Self {
        Self {
            credential,
            attribution,
            selected: None,
            generation: None,
            notices: vec![],
            tried: HashSet::new(),
        }
    }

    /// Whether the request goes to the subscription host rather than the API.
    pub fn is_subscription(&self) -> bool {
        matches!(self.credential, Credential::Bearer(_))
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
    config: &OpenaiConfig,
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
    config: &OpenaiConfig,
    store: Option<&CredentialStore>,
    model: &str,
    now: DateTime<Utc>,
) -> Result<Attempt, ResolveError> {
    resolve_skipping(config, store, model, now, HashSet::new(), &network_refresh).await
}

/// Resolve the chain, ignoring the entries in `tried`.
async fn resolve_skipping(
    config: &OpenaiConfig,
    store: Option<&CredentialStore>,
    model: &str,
    now: DateTime<Utc>,
    tried: HashSet<AuthEntry>,
    refresh: &Refresh,
) -> Result<Attempt, ResolveError> {
    // Each pass either resolves or retires one chain entry, so the walk cannot
    // cycle; the bound still ends the loop if a profile never settles.
    for _ in 0..=config.auth.len() {
        let snapshot = store.map(CredentialStore::load).transpose()?;
        let (landing, selected, generation, notices) =
            walk_chain(config, snapshot.as_ref(), model, now, &tried)?;

        let (profile, refresh_token) = match landing {
            Landing::Ready(credential, attribution) => {
                debug!(
                    entry = %selected,
                    mechanism = credential.kind(),
                    model,
                    skipped = notices.len(),
                    "Resolved provider credential."
                );

                return Ok(Attempt {
                    credential,
                    attribution,
                    selected: Some(selected),
                    generation,
                    notices,
                    tried,
                });
            }
            Landing::Stale {
                profile,
                refresh_token,
            } => (profile, refresh_token),
        };

        // Whatever this returns, the next pass re-walks the chain and reads the
        // state it left: a rotated token resolves, a retired profile is skipped
        // with a notice the walk produces itself.
        refresh_under_lock(store, &profile, &refresh_token, now, refresh).await?;
    }

    Err(ResolveError::ChainExhausted {
        reasons: vec!["every profile needs a fresh login".to_owned()],
    })
}

/// Record why the attempt's credential is out and move to the next one.
///
/// Returns `None` when there is no chain to advance or nothing further in it,
/// which the caller surfaces as the original, now-terminal error.
pub(super) async fn advance(
    config: &OpenaiConfig,
    store: Option<&CredentialStore>,
    spent: &Attempt,
    error: &StreamError,
    model: &str,
    now: DateTime<Utc>,
) -> Option<Attempt> {
    let selected = spent.selected.as_ref()?;

    record_outcome(store, selected, spent.generation, error, now);

    // Whether or not the store took the record, this entry is out for the rest
    // of the request. Carrying that in memory is what lets an `api_key`, which
    // has no stored profile to cool down, fall through to the entry behind it,
    // and what keeps a failed store write from stranding the request on a
    // credential already known to be spent.
    let mut tried = spent.tried.clone();
    tried.insert(selected.clone());

    // Re-resolution reads the state just recorded against the same instant, so
    // a cooldown that starts now is already in effect for this walk.
    // An exhausted chain is not a new failure to report: the caller surfaces
    // the error that prompted the switch.
    let mut attempt =
        match resolve_skipping(config, store, model, now, tried, &network_refresh).await {
            Ok(attempt) => attempt,
            Err(error) => {
                debug!(%error, "Credential chain exhausted after a failed request.");
                return None;
            }
        };

    attempt
        .notices
        .push(switch_notice(error, selected, attempt.selected.as_ref()));

    Some(attempt)
}

/// Bring a stale profile's access token up to date, holding the store's
/// mutation lock across the exchange.
///
/// Refresh tokens rotate, so two callers that read the same stale token and
/// both present it leave one holding a token the endpoint has already retired.
/// Two requests from one invocation are enough to reach that: a turn and the
/// title generation it spawns build their own providers and resolve
/// independently.
///
/// The document is re-read under the lock before anything is sent, so the
/// common case, another caller refreshing while this one waited, costs a lock
/// acquisition and no network call.
///
/// A failed write of the rotated tokens is returned rather than logged: the
/// refresh token this call presented is spent, and resolving again would
/// present it a second time.
async fn refresh_under_lock(
    store: Option<&CredentialStore>,
    profile: &str,
    presented: &str,
    now: DateTime<Utc>,
    refresh: &Refresh,
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

    match refresh(refresh_token).await {
        Ok(tokens) => rotate(&mut document, profile, &tokens),

        // The grant was refused: this profile cannot serve requests again
        // until the user logs in.
        Err(error) if error.is_rejection() => {
            warn!(%error, profile, "Refresh rejected; profile needs a fresh login.");
            retire(&mut document, profile);
        }

        // The endpoint could not be reached or is throttling. The credential
        // is probably fine, and so is the next attempt; falling to another
        // chain entry would not help, since the request itself needs the same
        // network.
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
/// `None` once another caller has rotated the token or moved the expiry out.
fn still_stale(
    document: &StoreDocument,
    profile: &str,
    presented: &str,
    now: DateTime<Utc>,
) -> Option<String> {
    match &document
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI)?
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
/// The profile keeps its generation: rotating a token renews the same
/// credential, and the outcome of a request already in flight under it is still
/// its own.
fn rotate(document: &mut StoreDocument, profile: &str, tokens: &oauth::Tokens) {
    let Some(stored) = document.profile_mut(CATEGORY_LLM, PROVIDER_OPENAI, profile) else {
        return;
    };

    stored.secret = CredentialSecret::Oauth {
        access_token: tokens.access_token.clone(),
        refresh_token: tokens.refresh_token.clone(),
        expires_at: tokens.expires_at,
    };
    stored.needs_relogin = false;

    // An import or an early login can leave identity unresolved; a refresh
    // carries the claims again, so fill it in when it is still missing.
    if stored.account_id.is_none() {
        stored.account_id.clone_from(&tokens.identity.account_id);
    }
    if stored.email.is_none() {
        stored.email.clone_from(&tokens.identity.email);
    }

    debug!(profile, "Rotated stored OAuth tokens.");
}

/// Mark a profile as needing a fresh login after its refresh was refused.
fn retire(document: &mut StoreDocument, profile: &str) {
    if let Some(stored) = document.profile_mut(CATEGORY_LLM, PROVIDER_OPENAI, profile) {
        stored.needs_relogin = true;
    }
}

/// Persist why the spent credential cannot serve the request.
///
/// Best-effort: a store that cannot be written costs this switch its
/// cross-invocation memory, not the request itself.
///
/// Each recorded state carries a cost, so it is keyed off the error's kind
/// rather than off "the request failed".
/// A cooldown takes a profile out of use for up to seven days and a re-login
/// marker until the user acts, and a failure that says nothing about the
/// credential earns neither.
fn record_outcome(
    store: Option<&CredentialStore>,
    spent: &AuthEntry,
    generation: Option<u64>,
    error: &StreamError,
    now: DateTime<Utc>,
) {
    // Only a stored profile has state to record against.
    let (AuthEntry::Subscription(Some(profile)), Some(generation)) = (spent, generation) else {
        return;
    };
    let Some(store) = store else {
        return;
    };

    let result = match error.kind {
        StreamErrorKind::AuthRejected => {
            debug!(profile, "Recording that the credential was refused.");
            store.mark_needs_relogin(CATEGORY_LLM, PROVIDER_OPENAI, profile, generation)
        }
        StreamErrorKind::SubscriptionExhausted | StreamErrorKind::InsufficientQuota => {
            let scope = error.quota_scope.as_deref().unwrap_or(SCOPE_ACCOUNT);
            let until = cooldown_until(error.quota_reset, now);
            debug!(profile, scope, %until, "Recording quota cooldown.");
            store.record_cooldown(
                CATEGORY_LLM,
                PROVIDER_OPENAI,
                profile,
                generation,
                scope,
                until,
            )
        }

        // The credential is fine; something else about the request was not.
        kind => {
            debug!(
                profile,
                ?kind,
                "Nothing to record: the failure does not implicate the credential."
            );
            return;
        }
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

/// The one-line notice announcing a credential switch.
fn switch_notice(error: &StreamError, from: &AuthEntry, to: Option<&AuthEntry>) -> String {
    let reason = if error.is_auth_rejected() {
        "rejected"
    } else if error.kind == StreamErrorKind::SubscriptionExhausted {
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
#[expect(clippy::too_many_lines)]
fn walk_chain(
    config: &OpenaiConfig,
    store: Option<&StoreDocument>,
    model: &str,
    now: DateTime<Utc>,
    tried: &HashSet<AuthEntry>,
) -> Result<(Landing, AuthEntry, Option<u64>, Vec<String>), ResolveError> {
    let mut notices = vec![];
    let mut reasons = vec![];

    for entry in &config.auth {
        // Settled here rather than in config, which sees neither source.
        let owned;
        let entry = match entry {
            AuthEntry::Named(name) => {
                owned = classify(&config.api_key_env, store, PROVIDER_OPENAI, name)?;
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
                        Landing::Ready(Credential::ApiKey(key), Attribution::default()),
                        entry.clone(),
                        None,
                        notices,
                    ));
                }

                // A single-entry chain reports the missing variable by name,
                // rather than as an exhausted chain of one.
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
                let (profile, stored) = lookup_profile(store, name.as_deref())?;

                // A bare `subscription` entry is reported as the profile it
                // resolved to, so callers and logs name a concrete credential.
                let selected = AuthEntry::Subscription(Some(profile.to_owned()));
                let generation = stored.generation;

                // Both spellings are checked: the chain may hold the bare entry
                // while the tried set holds the profile it resolved to.
                if tried.contains(entry) || tried.contains(&selected) {
                    skip(
                        &mut notices,
                        &mut reasons,
                        format!("{selected}: already tried for this request"),
                    );
                    continue;
                }

                // Not a fault of the credential, so nothing is recorded: the
                // same subscription serves the next request for another model.
                if super::is_api_only(model) {
                    skip(
                        &mut notices,
                        &mut reasons,
                        format!(
                            "{selected} does not serve {model}; only an `api_key` entry reaches it"
                        ),
                    );
                    continue;
                }

                if stored.needs_relogin {
                    skip(
                        &mut notices,
                        &mut reasons,
                        format!(
                            "{selected} needs re-login; run `jp provider llm auth login openai \
                             --name {profile}`"
                        ),
                    );
                    continue;
                }

                if let Some((scope, until)) = stored.active_cooldown(model, now) {
                    skip(
                        &mut notices,
                        &mut reasons,
                        format!("{selected} cooling down until {until} ({scope})"),
                    );
                    continue;
                }

                match &stored.secret {
                    CredentialSecret::Token { token } => {
                        return Ok((
                            Landing::Ready(
                                Credential::Bearer(token.clone()),
                                attribution(stored, token),
                            ),
                            selected,
                            Some(generation),
                            notices,
                        ));
                    }
                    CredentialSecret::Oauth {
                        access_token,
                        refresh_token,
                        expires_at,
                    } => {
                        // Refreshed slightly before the deadline, so a token
                        // cannot expire between being resolved and being used.
                        if *expires_at <= now + oauth::EXPIRY_BUFFER {
                            return Ok((
                                Landing::Stale {
                                    profile: profile.to_owned(),
                                    refresh_token: refresh_token.clone(),
                                },
                                selected,
                                Some(generation),
                                notices,
                            ));
                        }

                        return Ok((
                            Landing::Ready(
                                Credential::Bearer(access_token.clone()),
                                attribution(stored, access_token),
                            ),
                            selected,
                            Some(generation),
                            notices,
                        ));
                    }
                }
            }

            AuthEntry::Named(_) => unreachable!("classified above"),
        }
    }

    Err(ResolveError::ChainExhausted { reasons })
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

/// The attribution a subscription request carries for a stored profile.
///
/// The account id comes from the store, where login recorded it; the residency
/// constraint is a claim on the token in hand, so it is read fresh each time.
fn attribution(stored: &StoredCredential, token: &str) -> Attribution {
    Attribution {
        account_id: stored.account_id.clone(),
        residency: oauth::residency(token),
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
        .profiles(CATEGORY_LLM, PROVIDER_OPENAI);

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
