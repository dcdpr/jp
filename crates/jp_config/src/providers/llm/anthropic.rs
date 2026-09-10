//! Anthropic API configuration.

use std::{collections::HashSet, fmt, str::FromStr};

use schematic::{Config, ConfigError, HandlerError, Schema, SchemaBuilder, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_vec, delta_opt_vec_at, path},
    fill::FillDefaults,
    internal::merge::append_vec_dedup,
    partial::{ToPartial, partial_opt},
    validate::Validator,
};

/// Anthropic API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AnthropicConfig {
    /// The credential chain used to authenticate requests, in fallback order.
    ///
    /// Defaults to `["api_key"]`.
    ///
    /// Each entry selects a credential source:
    ///
    /// - `api_key`: The API key read from the environment variable named by
    ///   `api_key_env`.
    /// - `profile`: The sole stored credential profile.
    ///   Log in with `jp provider auth login llm.anthropic`.
    /// - `profile:<name>`: The stored credential profile `<name>`.
    ///   Profile names are case-sensitive.
    ///
    /// Entries are tried in order: when one cannot produce a usable credential,
    /// JP continues with the next.
    /// Listing `api_key` after subscription profiles authorizes continuing on
    /// per-token API billing when the subscription allowance is exhausted.
    ///
    /// This list replaces the one from earlier config layers as a whole; it
    /// never appends.
    ///
    /// ```toml
    /// [providers.llm.anthropic]
    /// auth = ["profile:personal", "profile:work", "api_key"]
    /// ```
    #[setting(default = vec![AuthEntry::ApiKey])]
    pub auth: Vec<AuthEntry>,

    /// Environment variable that contains the API key.
    #[setting(default = "ANTHROPIC_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[setting(default = "https://api.anthropic.com")]
    pub base_url: String,

    /// Whether to chain multiple requests when a request is stopped early due
    /// to exceeding the maximum number of tokens allowed by the model.
    ///
    /// This is enabled by default, but even when enabled, if you explicitly set
    /// the model's `max_tokens` parameter, the request will not be chained when
    /// that limit is reached.
    /// This allows for better cost control.
    #[setting(default = true)]
    pub chain_on_max_tokens: bool,

    /// Any optional headers to enable beta features.
    ///
    /// See: <https://docs.anthropic.com/en/api/beta-headers>
    ///
    /// To find out which beta headers are available, see:
    /// <https://docs.anthropic.com/en/release-notes/api>
    #[setting(default = vec![], merge = append_vec_dedup)]
    pub beta_headers: Vec<String>,
}

impl Validator for AnthropicConfig {
    /// Rejects an empty or duplicate-carrying `auth` chain.
    ///
    /// Unrecognized entries are rejected earlier, when the value is parsed into
    /// an [`AuthEntry`].
    fn validate(&self) -> Result<(), ConfigError> {
        if self.auth.is_empty() {
            return Err(HandlerError::new(
                "providers.llm.anthropic.auth must contain at least one entry, e.g. [\"api_key\"]",
            )
            .into());
        }

        let mut seen = HashSet::new();
        for entry in &self.auth {
            if !seen.insert(entry) {
                return Err(HandlerError::new(format!(
                    "providers.llm.anthropic.auth contains duplicate entry {entry}"
                ))
                .into());
            }
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialAnthropicConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "chain_on_max_tokens" => self.chain_on_max_tokens = kv.try_some_bool()?,
            _ if kv.p("auth") => {
                kv.try_some_vec(&mut self.auth, |kv| match kv.value.into_value() {
                    serde_json::Value::String(s) => s.parse().map_err(Into::into),
                    value => Err(format!("expected a string, got {value}").into()),
                })?;
            }
            _ if kv.p("beta_headers") => kv.try_some_vec_of_strings(&mut self.beta_headers)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAnthropicConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            chain_on_max_tokens: delta_opt(
                self.chain_on_max_tokens.as_ref(),
                next.chain_on_max_tokens,
            ),
            beta_headers: delta_opt_vec(self.beta_headers.as_ref(), next.beta_headers),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            // `auth` replaces rather than appends, so the whole of `next` is
            // reachable by merging and no path needs clearing first.
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            chain_on_max_tokens: delta_opt(
                self.chain_on_max_tokens.as_ref(),
                next.chain_on_max_tokens,
            ),
            beta_headers: delta_opt_vec_at(
                &path(prefix, "beta_headers"),
                self.beta_headers.as_ref(),
                next.beta_headers,
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialAnthropicConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auth: self.auth.or(defaults.auth),
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
            chain_on_max_tokens: self.chain_on_max_tokens.or(defaults.chain_on_max_tokens),
            beta_headers: self.beta_headers.or(defaults.beta_headers),
        }
    }
}

impl ToPartial for AnthropicConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            auth: partial_opt(&self.auth, defaults.auth),
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
            chain_on_max_tokens: partial_opt(
                &self.chain_on_max_tokens,
                defaults.chain_on_max_tokens,
            ),
            beta_headers: partial_opt(&self.beta_headers, defaults.beta_headers),
        }
    }
}

/// A single entry in the `auth` credential chain.
///
/// Written as a string in configuration files:
///
/// - `api_key`: The API key read from the environment variable named by
///   `api_key_env`.
/// - `profile`: The sole stored credential profile.
/// - `profile:<name>`: The stored credential profile `<name>`.
///   Profile names are case-sensitive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AuthEntry {
    /// Authenticate with the API key from the environment.
    ApiKey,

    /// Authenticate with a stored credential profile.
    ///
    /// `None` refers to the sole configured profile and is a preflight error
    /// when zero or multiple profiles are stored.
    Profile(Option<String>),
}

impl fmt::Display for AuthEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey => f.write_str("api_key"),
            Self::Profile(None) => f.write_str("profile"),
            Self::Profile(Some(name)) => write!(f, "profile:{name}"),
        }
    }
}

/// Error when parsing an [`AuthEntry`] from a string.
#[derive(Debug, thiserror::Error)]
#[error(
    "unrecognized auth chain entry: {0:?} (expected \"api_key\", \"profile\", or \
     \"profile:<name>\")"
)]
pub struct AuthEntryParseError(String);

impl FromStr for AuthEntry {
    type Err = AuthEntryParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "api_key" => Ok(Self::ApiKey),
            "profile" => Ok(Self::Profile(None)),
            _ => match s.strip_prefix("profile:") {
                Some(name) if !name.is_empty() => Ok(Self::Profile(Some(name.to_owned()))),
                _ => Err(AuthEntryParseError(s.to_owned())),
            },
        }
    }
}

impl Serialize for AuthEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AuthEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Schematic for AuthEntry {
    fn schema_name() -> Option<String> {
        Some("AuthEntry".into())
    }

    fn build_schema(mut schema: SchemaBuilder) -> Schema {
        schema.string_default()
    }
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod tests;
