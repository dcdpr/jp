//! LLM request behavior configuration.

use std::{fmt, time::Duration};

use schematic::Config;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Configuration for LLM request behavior.
///
/// Controls retry logic for transient errors like rate limits, timeouts, and
/// connection failures.
#[derive(Debug, Clone, Copy, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct RequestConfig {
    /// Maximum retry attempts for transient errors.
    ///
    /// Retryable errors include rate limits, timeouts, connection errors, and
    /// transient server errors (5xx). Set to 0 to disable retries.
    ///
    /// Non-retryable errors (auth failures, unknown models, invalid requests)
    /// are never retried regardless of this setting.
    #[setting(default = 5)]
    pub max_retries: u32,

    /// Base delay for exponential backoff (in milliseconds).
    ///
    /// The actual delay is calculated as:
    ///
    /// ```text
    /// delay = min(base_backoff * 2^attempt + jitter, max_backoff)
    /// ```
    ///
    /// Where jitter is a random value between 0-500ms to prevent thundering
    /// herd problems.
    #[setting(default = 1000)]
    pub base_backoff_ms: u32,

    /// Maximum backoff delay (in seconds).
    ///
    /// The backoff delay will never exceed this value, regardless of the number
    /// of retry attempts.
    #[setting(default = 60)]
    pub max_backoff_secs: u32,

    /// Prompt caching policy.
    ///
    /// Controls whether the provider should apply prompt caching optimizations
    /// (e.g., Anthropic's `cache_control` annotations).
    ///
    /// Accepts booleans (`true`/`false`) or strings (`"off"`, `"short"`,
    /// `"long"`).
    #[setting(default)]
    pub cache: CachePolicy,
}

impl AssignKeyValue for PartialRequestConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "max_retries" => self.max_retries = kv.try_some_u32()?,
            "base_backoff_ms" => self.base_backoff_ms = kv.try_some_u32()?,
            "max_backoff_secs" => self.max_backoff_secs = kv.try_some_u32()?,
            "cache" => self.cache = kv.try_some_bool_or_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialRequestConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            max_retries: delta_opt(self.max_retries.as_ref(), next.max_retries),
            base_backoff_ms: delta_opt(self.base_backoff_ms.as_ref(), next.base_backoff_ms),
            max_backoff_secs: delta_opt(self.max_backoff_secs.as_ref(), next.max_backoff_secs),
            cache: delta_opt(self.cache.as_ref(), next.cache),
        }
    }
}

impl ToPartial for RequestConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            max_retries: partial_opt(&self.max_retries, defaults.max_retries),
            base_backoff_ms: partial_opt(&self.base_backoff_ms, defaults.base_backoff_ms),
            max_backoff_secs: partial_opt(&self.max_backoff_secs, defaults.max_backoff_secs),
            cache: partial_opt(&self.cache, defaults.cache),
        }
    }
}

/// Controls whether the provider should apply prompt caching.
///
/// Providers map these values to their native caching mechanisms:
/// - Anthropic: `cache_control` annotations and automatic caching
/// - Other providers: provider-specific caching hints
///
/// When `Off`, the provider skips all caching annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CachePolicy {
    /// No caching. The provider skips all cache annotations.
    Off,

    /// Standard caching with provider-default TTL (typically ~5 minutes).
    #[default]
    Short,

    /// Extended caching with longer TTL (typically ~1 hour where supported).
    Long,

    /// Custom duration. Not all providers support arbitrary durations;
    /// unsupported values are rounded to the nearest available option.
    Custom(Duration),
}

impl CachePolicy {
    /// Returns `true` if caching is disabled.
    #[must_use]
    pub const fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }
}

impl From<bool> for CachePolicy {
    fn from(v: bool) -> Self {
        if v { Self::Short } else { Self::Off }
    }
}

impl std::str::FromStr for CachePolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "true" | "short" => Ok(Self::Short),
            "false" | "off" => Ok(Self::Off),
            "long" => Ok(Self::Long),
            _ => humantime::parse_duration(s).map(Self::Custom).map_err(|_| {
                format!(
                    "invalid cache policy: '{s}', expected one of: true, false, off, short, long, \
                     or a duration (e.g. '10m')"
                )
            }),
        }
    }
}

impl fmt::Display for CachePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => write!(f, "off"),
            Self::Short => write!(f, "short"),
            Self::Long => write!(f, "long"),
            Self::Custom(d) => write!(f, "{}", humantime::format_duration(*d)),
        }
    }
}

impl Serialize for CachePolicy {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Off => serializer.serialize_bool(false),
            Self::Short => serializer.serialize_bool(true),
            Self::Long => serializer.serialize_str("long"),
            Self::Custom(d) => {
                serializer.serialize_str(&humantime::format_duration(*d).to_string())
            }
        }
    }
}

impl<'de> Deserialize<'de> for CachePolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CachePolicyVisitor;

        impl serde::de::Visitor<'_> for CachePolicyVisitor {
            type Value = CachePolicy;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a boolean, one of \"off\"/\"short\"/\"long\", or a duration (e.g. \"10m\")",
                )
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<CachePolicy, E> {
                Ok(CachePolicy::from(v))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<CachePolicy, E> {
                v.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(CachePolicyVisitor)
    }
}

impl schematic::Schematic for CachePolicy {
    fn schema_name() -> Option<String> {
        Some("CachePolicy".to_owned())
    }

    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        use schematic::schema::{BooleanType, EnumType, LiteralValue, StringType, UnionType};

        schema.union(UnionType::new_any([
            schema.nest().boolean(BooleanType::default()),
            schema.nest().enumerable(EnumType::new([
                LiteralValue::String("off".into()),
                LiteralValue::String("short".into()),
                LiteralValue::String("long".into()),
            ])),
            schema.nest().string(StringType::default()),
        ]))
    }
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
