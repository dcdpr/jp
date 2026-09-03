//! Google API configuration.

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt, partial_opts},
};

/// Service tier for Gemini API requests.
///
/// - `standard`: Regular priority and standard pricing.
///   This is the default.
/// - `flex`: 50% discount for latency-tolerant workloads with best-effort
///   availability.
/// - `priority`: Premium tier with highest priority handling and lowest
///   latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum ServiceTier {
    /// 50% discount for latency-tolerant workloads with best-effort
    /// availability.
    Flex,
    /// Premium tier with highest priority handling and lowest latency.
    Priority,
    /// Regular priority and standard pricing.
    Standard,
}

/// Google API configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct GoogleConfig {
    /// Environment variable that contains the API key.
    #[setting(default = "GEMINI_API_KEY")]
    pub api_key_env: String,

    /// The base URL to use for API requests.
    #[setting(default = "https://generativelanguage.googleapis.com/v1beta")]
    pub base_url: String,

    /// Service tier for Gemini API requests.
    ///
    /// - `standard`: Regular priority and pricing.
    ///   This is the default.
    /// - `flex`: 50% discount for latency-tolerant workloads with best-effort
    ///   availability.
    /// - `priority`: Premium tier with highest priority handling and lowest
    ///   latency.
    ///
    /// Defaults to `standard` if unset.
    pub service_tier: Option<ServiceTier>,
}

impl AssignKeyValue for PartialGoogleConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "api_key_env" => self.api_key_env = kv.try_some_string()?,
            "base_url" => self.base_url = kv.try_some_string()?,
            "service_tier" => self.service_tier = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialGoogleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            api_key_env: delta_opt(self.api_key_env.as_ref(), next.api_key_env),
            base_url: delta_opt(self.base_url.as_ref(), next.base_url),
            service_tier: delta_opt(self.service_tier.as_ref(), next.service_tier),
        }
    }
}

impl FillDefaults for PartialGoogleConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            api_key_env: self.api_key_env.or(defaults.api_key_env),
            base_url: self.base_url.or(defaults.base_url),
            service_tier: self.service_tier.or(defaults.service_tier),
        }
    }
}

impl ToPartial for GoogleConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            api_key_env: partial_opt(&self.api_key_env, defaults.api_key_env),
            base_url: partial_opt(&self.base_url, defaults.base_url),
            service_tier: partial_opts(self.service_tier.as_ref(), defaults.service_tier),
        }
    }
}
