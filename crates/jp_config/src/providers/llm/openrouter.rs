//! Openrouter API configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt, partial_opts},
};

/// Openrouter API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct OpenrouterConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "OPENROUTER_API_KEY")]
    pub api_key_env: String,

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

impl AssignKeyValue for PartialOpenrouterConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
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
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            app_name: delta_opt(self.app_name.as_ref(), next.app_name),
            app_referrer: delta_opt(self.app_referrer.as_ref(), next.app_referrer),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
        }
    }
}

impl ToPartial for OpenrouterConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            app_name: partial_opt(&self.app_name, defaults.app_name),
            app_referrer: partial_opts(self.app_referrer.as_ref(), defaults.app_referrer),
            base_url: partial_opt(&self.base_url, defaults.base_url),
        }
    }
}
