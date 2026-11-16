//! Google API configuration.

use schematic::{Config, ConfigError};
use serde_json::Value;

pub use crate::providers::llm::{AuthEntry, AuthEntryParseError};
use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    types::api_key_env::ApiKeyEnv,
    validate::Validator,
};

/// The configuration path the credential chain lives at.
const AUTH_KEY: &str = "providers.llm.google.auth";

/// Google API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct GoogleConfig {
    /// The credential chain used to authenticate requests, in fallback order.
    ///
    /// Defaults to `["api_key"]`.
    ///
    /// - `api_key`: Metered billing, using the key `api_key_env` names.
    /// - `api_key:<name>`: Metered billing with the named key, when
    ///   `api_key_env` maps several.
    ///
    /// `api` is accepted as shorthand for the kind, and a bare name selects the
    /// key that answers to it.
    /// Names are case-sensitive.
    ///
    /// Google has no subscription plan JP can authenticate against, so a
    /// `subscription` entry is a resolution error.
    ///
    /// Entries are tried in order: when one cannot produce a usable credential,
    /// JP continues with the next.
    ///
    /// ```toml
    /// [providers.llm.google]
    /// auth = ["api_key:work", "api_key:personal"]
    /// ```
    #[setting(default = vec![AuthEntry::ApiKey(None)])]
    pub auth: Vec<AuthEntry>,

    /// Environment variable that contains the API key.
    ///
    /// A map names several keys, each selectable from the `auth` chain as
    /// `api_key:<name>`.
    /// Each key is a variable, or a list read the same way as below:
    ///
    /// ```toml
    /// api_key_env = { work = "WORK_GEMINI_KEY", personal = "MY_GEMINI_KEY" }
    /// ```
    ///
    /// A list is one key, read from the first variable that holds a non-empty
    /// value:
    ///
    /// ```toml
    /// api_key_env = ["WORK_GEMINI_KEY", "MY_GEMINI_KEY"]
    /// ```
    #[setting(default = "GEMINI_API_KEY")]
    pub api_key_env: ApiKeyEnv,

    /// The base URL to use for API requests.
    #[setting(default = "https://generativelanguage.googleapis.com/v1beta")]
    pub base_url: String,

    /// How many seconds Google may send nothing while working on a response,
    /// before `assistant.request.stream_idle_timeout_secs` starts counting.
    ///
    /// Defaults to `300`.
    /// Set to `0` to apply only the idle timeout, as for other providers.
    ///
    /// Google sends nothing, not even response headers, until the first part of
    /// a response exists, and it sends a tool call only once all of its
    /// arguments are written.
    /// A response to a large conversation, or one that opens with a large tool
    /// call, can therefore be silent for minutes while healthy.
    /// Each attempt that times out resends the whole conversation and is billed
    /// again.
    ///
    /// The allowance applies to every silent stretch, before the first part and
    /// between parts.
    /// A connection that has died is detected after this allowance plus one
    /// idle timeout.
    #[setting(default = 300)]
    pub silence_allowance_secs: u32,
}

impl Validator for GoogleConfig {
    /// Rejects an empty or duplicate-carrying `auth` chain.
    ///
    /// Unrecognized entries are rejected earlier, when the value is parsed into
    /// an [`AuthEntry`].
    fn validate(&self) -> Result<(), ConfigError> {
        AuthEntry::validate_chain(&self.auth, AUTH_KEY)
    }
}

impl AssignKeyValue for PartialGoogleConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("auth") => {
                kv.try_some_vec(&mut self.auth, |kv| match kv.value.into_value() {
                    Value::String(s) => s.parse::<AuthEntry>().map_err(Into::into),
                    value => Err(format!("expected a string, got {value}").into()),
                })?;
            }
            "api_key_env" => self.api_key_env = kv.try_some_value()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "silence_allowance_secs" => self.silence_allowance_secs = kv.try_some_u32()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialGoogleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            silence_allowance_secs: delta_opt(
                self.silence_allowance_secs.as_ref(),
                next.silence_allowance_secs,
            ),
        }
    }
}

impl FillDefaults for PartialGoogleConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auth: self.auth.or(defaults.auth),
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
            silence_allowance_secs: self
                .silence_allowance_secs
                .or(defaults.silence_allowance_secs),
        }
    }
}

impl ToPartial for GoogleConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            auth: partial_opt(&self.auth, defaults.auth),
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
            silence_allowance_secs: partial_opt(
                &self.silence_allowance_secs,
                defaults.silence_allowance_secs,
            ),
        }
    }
}
