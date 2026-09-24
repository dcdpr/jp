//! Openrouter API configuration.

use schematic::{Config, ConfigError};
use serde_json::Value;

pub use crate::providers::llm::{AuthEntry, AuthEntryParseError};
use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_at, path},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt, partial_opts},
    types::api_key_env::ApiKeyEnv,
    validate::Validator,
};

/// The configuration path the credential chain lives at.
const AUTH_KEY: &str = "providers.llm.openrouter.auth";

/// Openrouter API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct OpenrouterConfig {
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
    /// Openrouter has no subscription plan JP can authenticate against, so a
    /// `subscription` entry is a resolution error.
    ///
    /// Entries are tried in order: when one cannot produce a usable credential,
    /// JP continues with the next.
    ///
    /// ```toml
    /// [providers.llm.openrouter]
    /// auth = ["api_key:work", "api_key:personal"]
    /// ```
    #[setting(default = vec![AuthEntry::ApiKey(None)])]
    pub auth: Vec<AuthEntry>,

    /// Environment variable that contains the API key.
    ///
    /// A map names several keys, each selectable from the `auth` chain as
    /// `api_key:<name>`:
    ///
    /// ```toml
    /// api_key_env = { work = "WORK_OPENROUTER_KEY", personal = "MY_OPENROUTER_KEY" }
    /// ```
    #[setting(default = "OPENROUTER_API_KEY")]
    pub api_key_env: ApiKeyEnv,

    /// Application name sent to Openrouter.
    #[setting(default = "JP")]
    pub app_name: String,

    /// Optional HTTP referrer to send with requests.
    ///
    /// This is used by Openrouter to identify the application.
    pub app_referrer: Option<String>,

    /// The base URL to use for API requests.
    #[setting(default = "https://openrouter.ai")]
    pub base_url: String,
}

impl Validator for OpenrouterConfig {
    /// Rejects an empty or duplicate-carrying `auth` chain.
    ///
    /// Unrecognized entries are rejected earlier, when the value is parsed into
    /// an [`AuthEntry`].
    fn validate(&self) -> Result<(), ConfigError> {
        AuthEntry::validate_chain(&self.auth, AUTH_KEY)
    }
}

impl AssignKeyValue for PartialOpenrouterConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("auth") => {
                kv.try_some_vec(&mut self.auth, |kv| match kv.value.into_value() {
                    Value::String(s) => s.parse::<AuthEntry>().map_err(Into::into),
                    value => Err(format!("expected a string, got {value}").into()),
                })?;
            }
            "api_key_env" => self.api_key_env = kv.try_some_object_or_from_str()?,
            "app_name" => self.app_name = kv.try_some_string()?,
            "app_referrer" => self.app_referrer = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialOpenrouterConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            app_name: delta_opt(self.app_name.as_ref(), next.app_name),
            app_referrer: delta_opt(self.app_referrer.as_ref(), next.app_referrer),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            auth: delta_opt(self.auth.as_ref(), next.auth),
            api_key_env: delta_opt_at(
                &path(prefix, "api_key_env"),
                self.api_key_env.as_ref(),
                next.api_key_env,
                unsets,
            ),
            app_name: delta_opt_at(
                &path(prefix, "app_name"),
                self.app_name.as_ref(),
                next.app_name,
                unsets,
            ),
            app_referrer: delta_opt_at(
                &path(prefix, "app_referrer"),
                self.app_referrer.as_ref(),
                next.app_referrer,
                unsets,
            ),
            base_url: delta_opt_at(
                &path(prefix, "base_url"),
                self.base_url.as_ref(),
                next.base_url,
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialOpenrouterConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            auth: self.auth.or(defaults.auth),
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            app_name: self.app_name.or(defaults.app_name),
            app_referrer: self.app_referrer.or(defaults.app_referrer),
            base_url: self.base_url.or(defaults.base_url),
        }
    }
}

impl ToPartial for OpenrouterConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            auth: partial_opt(&self.auth, defaults.auth),
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            app_name: partial_opt(&self.app_name, defaults.app_name),
            app_referrer: partial_opts(self.app_referrer.as_ref(), defaults.app_referrer),
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
