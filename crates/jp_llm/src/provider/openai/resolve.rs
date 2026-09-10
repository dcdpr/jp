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
//! cooldown, and the recorded state is what routes the next resolution past it
//! — a mid-turn switch and a fresh invocation share one code path.

use chrono::{DateTime, Utc};
use jp_config::{
    providers::llm::{AuthEntry, openai::OpenaiConfig},
    types::api_key_env::ApiKeyEnv,
};
use jp_credentials::{
    CATEGORY_LLM, CredentialSecret, CredentialStore, PROVIDER_OPENAI, SCOPE_ACCOUNT, StoreDocument,
    StoreError, StoredCredential, cooldown_until,
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

    /// A `profile:<name>` entry names a profile that is not stored.
    #[error(
        "providers.llm.openai.auth entry `profile:{name}` matches no stored profile; run `jp \
         provider llm auth login openai --name {name}` to create it"
    )]
    UnknownProfile {
        /// The profile name the chain entry refers to.
        name: String,
    },

    /// A bare `profile` entry with zero stored profiles.
    #[error(
        "providers.llm.openai.auth entry `profile` matches no stored profile; run `jp provider \
         llm auth login openai` to create one"
    )]
    NoProfiles,

    /// A bare `profile` entry with multiple stored profiles.
    #[error(
        "providers.llm.openai.auth entry `profile` is ambiguous: multiple profiles are stored \
         ({}); name one with `profile:<name>`",
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

    /// Which chain entry produced the credential, with a bare `profile`
    /// normalized to the profile it resolved to.
    ///
    /// `None` when the credential was injected directly instead of resolved
    /// from the chain, in which case there is no chain to advance.
    pub selected: Option<AuthEntry>,

    /// User-facing notices for skipped entries, surfaced as chrome.
    pub notices: Vec<String>,
}

impl Attempt {
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
    walk_chain(config, snapshot.as_ref(), model, now).map(drop)
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
    // Each pass either resolves or retires one chain entry, so the walk cannot
    // cycle; the bound is belt against a profile that refuses to settle.
    for _ in 0..=config.auth.len() {
        let snapshot = store.map(CredentialStore::load).transpose()?;
        let (landing, selected, mut notices) = walk_chain(config, snapshot.as_ref(), model, now)?;

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
                    notices,
                });
            }
            Landing::Stale {
                profile,
                refresh_token,
            } => (profile, refresh_token),
        };

        debug!(profile, "Access token expired; refreshing.");

        match oauth::refresh(&refresh_token).await {
            Ok(tokens) => persist_rotation(store, &profile, &refresh_token, &tokens),

            // The grant was refused: this profile cannot serve requests again
            // until the user logs in. Retire it and walk on.
            Err(error) if error.is_rejection() => {
                warn!(%error, profile, "Refresh rejected; profile needs a fresh login.");
                retire(store, &profile);

                notices.push(format!(
                    "skipping profile:{profile}: session expired; run `jp provider llm auth login \
                     openai --name {profile}`"
                ));
            }

            // The endpoint could not be reached. The credential is probably
            // fine, and so is the next attempt; falling to another chain entry
            // would not help, since the request itself needs the same network.
            Err(source) => return Err(ResolveError::Refresh { profile, source }),
        }
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
    spent: &AuthEntry,
    error: &StreamError,
    model: &str,
    now: DateTime<Utc>,
) -> Option<Attempt> {
    record_outcome(store, spent, error, now);

    // Re-resolution reads the state just recorded against the same instant, so
    // a cooldown that starts now is already in effect for this walk.
    // An exhausted chain is not a new failure to report: the caller surfaces
    // the error that prompted the switch.
    let mut attempt = match resolve(config, store, model, now).await {
        Ok(attempt) => attempt,
        Err(error) => {
            debug!(%error, "Credential chain exhausted after a failed request.");
            return None;
        }
    };

    // Resolution landing on the same entry means nothing was recorded that
    // could move it: `api_key` has no store entry, so an exhausted key falls
    // through instead of being retried forever.
    if attempt.selected.as_ref() == Some(spent) {
        debug!("Credential chain did not advance; treating the failure as terminal.");
        return None;
    }

    attempt
        .notices
        .push(switch_notice(error, spent, attempt.selected.as_ref()));

    Some(attempt)
}

/// Store the tokens a refresh produced.
///
/// Refresh tokens rotate, so the write rechecks its precondition under the
/// lock: if the stored token is no longer the one this refresh consumed,
/// another process rotated first and its tokens are the live ones.
/// Overwriting them would invalidate a credential that works.
fn persist_rotation(
    store: Option<&CredentialStore>,
    profile: &str,
    consumed: &str,
    tokens: &oauth::Tokens,
) {
    let Some(store) = store else {
        return;
    };

    let result = store.mutate(|document| {
        let Some(stored) = document.profile_mut(CATEGORY_LLM, PROVIDER_OPENAI, profile) else {
            return Ok(false);
        };

        if !holds_refresh_token(stored, consumed) {
            debug!(
                profile,
                "Another process rotated first; keeping its tokens."
            );
            return Ok(false);
        }

        stored.secret = CredentialSecret::Oauth {
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: tokens.expires_at,
        };
        stored.needs_relogin = false;

        // An import or an early login can leave identity unresolved; a refresh
        // carries the claims again, so fill it in when it is still missing.
        if stored.account_id.is_none() {
            stored.account_id = tokens.identity.account_id.clone();
        }
        if stored.email.is_none() {
            stored.email = tokens.identity.email.clone();
        }

        Ok(true)
    });

    match result {
        Ok(true) => {}
        Ok(false) => debug!(profile, "Rotated tokens were not persisted."),
        Err(error) => warn!(%error, profile, "Could not persist rotated tokens."),
    }
}

/// Mark a profile as needing a fresh login.
fn retire(store: Option<&CredentialStore>, profile: &str) {
    let Some(store) = store else {
        return;
    };

    if let Err(error) = store.mark_needs_relogin(CATEGORY_LLM, PROVIDER_OPENAI, profile) {
        warn!(%error, profile, "Could not mark the profile as needing re-login.");
    }
}

/// Whether the stored credential still holds the refresh token a refresh
/// consumed.
fn holds_refresh_token(credential: &StoredCredential, token: &str) -> bool {
    match &credential.secret {
        CredentialSecret::Oauth { refresh_token, .. } => refresh_token == token,
        CredentialSecret::Token { .. } => false,
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
    error: &StreamError,
    now: DateTime<Utc>,
) {
    // Only a stored profile has state to record against.
    let AuthEntry::Subscription(Some(profile)) = spent else {
        return;
    };
    let Some(store) = store else {
        return;
    };

    let result = match error.kind {
        StreamErrorKind::AuthRejected => {
            debug!(profile, "Recording that the credential was refused.");
            store.mark_needs_relogin(CATEGORY_LLM, PROVIDER_OPENAI, profile)
        }
        StreamErrorKind::SubscriptionExhausted | StreamErrorKind::InsufficientQuota => {
            let scope = error.quota_scope.as_deref().unwrap_or(SCOPE_ACCOUNT);
            let until = cooldown_until(error.quota_reset, now);
            debug!(profile, scope, %until, "Recording quota cooldown.");
            store.record_cooldown(CATEGORY_LLM, PROVIDER_OPENAI, profile, scope, until)
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

    match result {
        Ok(true) => {}
        Ok(false) => warn!(
            profile,
            "Credential profile vanished before its state was recorded."
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
fn walk_chain(
    config: &OpenaiConfig,
    store: Option<&StoreDocument>,
    model: &str,
    now: DateTime<Utc>,
) -> Result<(Landing, AuthEntry, Vec<String>), ResolveError> {
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
                        notices,
                    ));
                }

                // Under the single-entry default chain, a missing key is the
                // same failure it was before chains existed.
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

                if stored.needs_relogin {
                    skip(
                        &mut notices,
                        &mut reasons,
                        format!(
                            "profile:{profile} needs re-login; run `jp provider llm auth login \
                             openai --name {profile}`"
                        ),
                    );
                    continue;
                }

                if let Some((scope, until)) = stored.active_cooldown(model, now) {
                    skip(
                        &mut notices,
                        &mut reasons,
                        format!("profile:{profile} cooling down until {until} ({scope})"),
                    );
                    continue;
                }

                // A bare `profile` entry is reported as the profile it resolved
                // to, so callers and logs name a concrete credential.
                let selected = AuthEntry::Subscription(Some(profile.to_owned()));

                match &stored.secret {
                    CredentialSecret::Token { token } => {
                        return Ok((
                            Landing::Ready(
                                Credential::Bearer(token.clone()),
                                attribution(stored, token),
                            ),
                            selected,
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
                                notices,
                            ));
                        }

                        return Ok((
                            Landing::Ready(
                                Credential::Bearer(access_token.clone()),
                                attribution(stored, access_token),
                            ),
                            selected,
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
/// A named entry must exist; a bare `profile` entry refers to the sole stored
/// profile and errors on zero or multiple candidates.
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
