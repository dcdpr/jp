//! Credential resolution for a provider that only reads API keys.
//!
//! Walks the provider's `auth` chain and takes the first key the environment
//! holds.
//!
//! Nothing here is stateful: there is no store to read, no token to refresh,
//! and no cooldown to record.
//! A provider with a subscription writes its own resolver instead, as Anthropic
//! and OpenAI do.

use std::env;

use jp_config::{
    providers::llm::AuthEntry,
    types::api_key_env::{ApiKeyEnv, ApiKeyEnvError},
};
use tracing::debug;

/// Read an API key from the environment.
///
/// A blank value reads as absent: `export K=` would otherwise send an empty
/// `Authorization` header and fail as a bad key.
///
/// A non-blank value is returned byte-exact, trailing newline and all.
#[must_use]
pub fn read_key(variable: &str) -> Option<String> {
    env::var(variable).ok().filter(|key| !key.trim().is_empty())
}

/// Why an API-key chain produced no credential.
#[derive(Debug, thiserror::Error)]
pub enum ChainError {
    /// A single-entry chain whose variable holds no key.
    #[error("Missing environment variable: {0}")]
    MissingEnv(String),

    /// A `subscription` entry against a provider that has none.
    #[error(
        "providers.llm.{provider}.auth entry `{entry}` asks for a subscription, and {provider} \
         has none JP can authenticate against; use `api_key` instead"
    )]
    NoSubscription {
        /// The provider whose chain names the entry.
        provider: String,

        /// The entry as the user wrote it.
        entry: String,
    },

    /// An entry naming a key `api_key_env` does not configure.
    #[error("providers.llm.{provider}.auth: {source}")]
    UnknownKey {
        /// The provider whose chain names the key.
        provider: String,

        /// What the lookup reported, including the configured names.
        #[source]
        source: ApiKeyEnvError,
    },

    /// Every entry in a multi-entry chain was skipped.
    #[error(
        "no usable credential in the providers.llm.{provider}.auth chain: {}",
        .skipped.join("; ")
    )]
    Exhausted {
        /// The provider whose chain resolved nothing.
        provider: String,

        /// Each entry that was skipped, and why.
        skipped: Vec<String>,
    },
}

/// Resolve the first usable API key in `chain`, and the entry it came from.
///
/// # Errors
///
/// A single-entry chain returns [`ChainError::MissingEnv`], matching what a
/// bare environment read reported before chains existed; a longer one returns
/// [`ChainError::Exhausted`] naming every entry it skipped.
///
/// A `subscription` entry, or a name no configured key answers to, is an error
/// rather than a skip.
pub(crate) fn resolve(
    provider: &str,
    chain: &[AuthEntry],
    keys: &ApiKeyEnv,
) -> Result<(String, AuthEntry), ChainError> {
    let mut skipped = vec![];

    for entry in chain {
        let name = match entry {
            AuthEntry::ApiKey(name) => name.as_deref(),

            // With no store to consult, a bare name can only be a key.
            AuthEntry::Named(name) => Some(name.as_str()),

            AuthEntry::Subscription(_) => {
                return Err(ChainError::NoSubscription {
                    provider: provider.to_owned(),
                    entry: entry.to_string(),
                });
            }
        };

        // A name no key answers to is a config mistake, not a credential to
        // fall past: the next entry would bill a different key.
        let variable = keys
            .variable(name)
            .map_err(|source| ChainError::UnknownKey {
                provider: provider.to_owned(),
                source,
            })?;

        if let Some(key) = read_key(variable) {
            return Ok((key, AuthEntry::ApiKey(name.map(str::to_owned))));
        }

        if chain.len() == 1 {
            return Err(ChainError::MissingEnv(variable.to_owned()));
        }

        debug!(provider, %entry, variable, "Skipping API key with no value.");
        skipped.push(format!("{entry}: {variable} is not set"));
    }

    Err(ChainError::Exhausted {
        provider: provider.to_owned(),
        skipped,
    })
}

#[cfg(test)]
#[path = "api_key_chain_tests.rs"]
mod tests;
