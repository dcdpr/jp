//! LLM request behavior configuration.

use std::{fmt, time::Duration};

use schematic::{Config, ConfigError, HandlerError, TransformResult};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    validate::Validator,
};

/// Minimum non-zero value accepted for `stream_idle_timeout_secs`.
///
/// Windows below this abort healthy streams (for example during slow tool-call
/// argument generation), so a resolved config rejects `1..MIN` in favor of `0`
/// (disabled) or a value at or above it.
pub const MIN_STREAM_IDLE_TIMEOUT_SECS: u32 = 10;

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
    /// transient server errors (5xx).
    /// Set to 0 to disable retries.
    ///
    /// Non-retryable errors (auth failures, unknown models, invalid requests)
    /// are never retried regardless of this setting.
    #[setting(default = 5)]
    pub max_retries: u32,

    /// Base delay for exponential backoff (in milliseconds).
    ///
    /// Defaults to `1000`.
    /// The first retry waits this long, and each further retry doubles it:
    ///
    /// ```text
    /// delay_ms = min(base_backoff_ms * 2^(attempt - 1), max_backoff_secs * 1000) + jitter
    /// ```
    ///
    /// Jitter is a random extra of up to a quarter of the delay.
    /// It keeps several JP sessions sharing one API key from resuming in the
    /// same instant after they are rate limited together, which would just
    /// trigger the limit again.
    ///
    /// When the provider says how long to wait, that delay is used instead of
    /// the doubling one, and still gets the jitter.
    #[setting(default = 1000)]
    pub base_backoff_ms: u32,

    /// Maximum backoff delay (in seconds).
    ///
    /// Defaults to `60`.
    /// Caps both the doubling delay and a wait the provider asked for, no
    /// matter how many attempts have been made.
    /// Jitter is added on top of the cap, so an individual wait can run up to a
    /// quarter longer than this.
    #[setting(default = 60)]
    pub max_backoff_secs: u32,

    /// Abort a streaming response after this many seconds of inactivity.
    ///
    /// Defaults to `60`.
    /// Set to `0` to disable the idle timeout; any other value must be at least
    /// `10`, since smaller windows abort healthy streams.
    ///
    /// The timer resets every time the provider sends data, so it only fires
    /// when a connection goes silent for the full duration without delivering
    /// any tokens or events.
    /// A timed-out stream is treated as a transient error and retried according
    /// to `max_retries` and the backoff settings.
    ///
    /// Raise this for models with a very long time-to-first-token (such as
    /// deep-research models) if you observe spurious retries.
    #[setting(default = 60)]
    pub stream_idle_timeout_secs: u32,

    /// Abort a response after it generates more than this many bytes.
    ///
    /// Defaults to `1048576` (1 MiB, roughly 260,000 tokens).
    /// Accepted values:
    ///
    /// - a byte count such as `4096`
    /// - `0`, `"disabled"`, `"off"`, or `false`: no ceiling
    ///
    /// This is a runaway guard rather than a length preference.
    /// It exists so a model that gets stuck generating without end cannot run
    /// up an unbounded bill while nobody is watching the terminal.
    /// The default allows long responses assembled from several continuation
    /// requests while bounding the cost of a runaway response.
    ///
    /// During a query, content streamed before the ceiling is reached stays in
    /// the conversation and only the turn ends, with an error.
    /// Background requests that collect a whole response before using it (title
    /// generation, summarization, tool inquiries) keep no partial result: they
    /// fail outright.
    ///
    /// Inquiry requests can be held to their own ceiling via
    /// `conversation.inquiry.assistant.request.max_response_bytes`, or per
    /// question via a question's assistant target.
    ///
    /// The ceiling counts bytes rather than tokens because tokens cannot be
    /// counted locally; four bytes per token is a rough guide.
    /// It counts the bytes JP receives, which can be fewer than the bytes
    /// billed: a provider that assembles a response from several continuation
    /// requests may discard some of what it generated before returning it.
    #[setting(default = default_max_response_bytes)]
    pub max_response_bytes: MaxResponseBytes,

    /// Prompt caching policy.
    ///
    /// Controls whether the provider applies prompt caching optimizations (e.g.
    /// Anthropic's `cache_control` annotations).
    ///
    /// Defaults to `short`.
    /// Accepted values:
    ///
    /// - `false` / `off`: Disable caching.
    /// - `true` / `short`: Cache with the provider's default short TTL
    ///   (typically ~5 minutes).
    /// - `long`: Cache with the provider's extended TTL (typically ~1 hour
    ///   where supported).
    /// - a duration such as `10m` or `1h`: Request that exact TTL.
    ///   Providers that don't support arbitrary durations round to the nearest
    ///   available option.
    #[setting(default)]
    pub cache: CachePolicy,
}

impl Validator for RequestConfig {
    /// Rejects a non-zero `stream_idle_timeout_secs` below
    /// [`MIN_STREAM_IDLE_TIMEOUT_SECS`]; `0` disables the timeout.
    fn validate(&self) -> Result<(), ConfigError> {
        if (1..MIN_STREAM_IDLE_TIMEOUT_SECS).contains(&self.stream_idle_timeout_secs) {
            return Err(HandlerError::new(format!(
                "assistant.request.stream_idle_timeout_secs must be 0 (disabled) or at least \
                 {MIN_STREAM_IDLE_TIMEOUT_SECS}, got {}",
                self.stream_idle_timeout_secs,
            ))
            .into());
        }

        Ok(())
    }
}

impl AssignKeyValue for PartialRequestConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "max_retries" => self.max_retries = kv.try_some_u32()?,
            "base_backoff_ms" => self.base_backoff_ms = kv.try_some_u32()?,
            "max_backoff_secs" => self.max_backoff_secs = kv.try_some_u32()?,
            "stream_idle_timeout_secs" => {
                self.stream_idle_timeout_secs = kv.try_some_u32()?;
            }
            "max_response_bytes" => {
                self.max_response_bytes = kv.try_some_bool_number_or_from_str()?;
            }
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
            stream_idle_timeout_secs: delta_opt(
                self.stream_idle_timeout_secs.as_ref(),
                next.stream_idle_timeout_secs,
            ),
            max_response_bytes: delta_opt(
                self.max_response_bytes.as_ref(),
                next.max_response_bytes,
            ),
            cache: delta_opt(self.cache.as_ref(), next.cache),
        }
    }
}

impl FillDefaults for PartialRequestConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            max_retries: self.max_retries.or(defaults.max_retries),
            base_backoff_ms: self.base_backoff_ms.or(defaults.base_backoff_ms),
            max_backoff_secs: self.max_backoff_secs.or(defaults.max_backoff_secs),
            stream_idle_timeout_secs: self
                .stream_idle_timeout_secs
                .or(defaults.stream_idle_timeout_secs),
            max_response_bytes: self.max_response_bytes.or(defaults.max_response_bytes),
            cache: self.cache.or(defaults.cache),
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
            stream_idle_timeout_secs: partial_opt(
                &self.stream_idle_timeout_secs,
                defaults.stream_idle_timeout_secs,
            ),
            max_response_bytes: partial_opt(&self.max_response_bytes, defaults.max_response_bytes),
            cache: partial_opt(&self.cache, defaults.cache),
        }
    }
}

/// A ceiling on the bytes of generated content a single response may produce.
///
/// The ceiling is a runaway guard: it bounds what a model that never stops
/// generating can cost.
/// `Disabled` removes the bound entirely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxResponseBytes {
    /// No ceiling.
    /// A response may generate without bound.
    Disabled,

    /// Abort the response once it generates more than this many bytes.
    Bytes(u32),
}

/// 1 MiB, roughly 260,000 tokens.
const DEFAULT_MAX_RESPONSE_BYTES: u32 = 1_048_576;

/// The default output ceiling.
#[expect(clippy::trivially_copy_pass_by_ref, clippy::unnecessary_wraps)]
const fn default_max_response_bytes(_: &()) -> TransformResult<Option<MaxResponseBytes>> {
    Ok(Some(MaxResponseBytes::Bytes(DEFAULT_MAX_RESPONSE_BYTES)))
}

impl Default for MaxResponseBytes {
    fn default() -> Self {
        Self::Bytes(DEFAULT_MAX_RESPONSE_BYTES)
    }
}

impl MaxResponseBytes {
    /// Build a ceiling from a byte count, where `0` disables it.
    ///
    /// A zero-byte ceiling would abort every response before its first event,
    /// so `0` reads as [`Self::Disabled`] instead, matching the sibling
    /// settings that spell "off" the same way.
    #[must_use]
    pub const fn from_bytes(bytes: u32) -> Self {
        match bytes {
            0 => Self::Disabled,
            bytes => Self::Bytes(bytes),
        }
    }

    /// The ceiling in bytes, or `None` when no ceiling applies.
    #[must_use]
    pub const fn bytes(self) -> Option<u64> {
        match self {
            Self::Disabled => None,
            Self::Bytes(bytes) => Some(bytes as u64),
        }
    }

    /// Returns `true` if no ceiling applies.
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        matches!(self, Self::Disabled)
    }
}

impl From<bool> for MaxResponseBytes {
    /// `false` disables the ceiling; `true` selects the default ceiling.
    fn from(v: bool) -> Self {
        if v { Self::default() } else { Self::Disabled }
    }
}

impl std::str::FromStr for MaxResponseBytes {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "disabled" | "off" | "false" => Ok(Self::Disabled),
            "true" => Ok(Self::default()),
            _ => s.parse::<u32>().map(Self::from_bytes).map_err(|_| {
                format!(
                    "invalid max response bytes: '{s}', expected a byte count or one of: \
                     disabled, off, false"
                )
            }),
        }
    }
}

impl fmt::Display for MaxResponseBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => write!(f, "disabled"),
            Self::Bytes(bytes) => write!(f, "{bytes}"),
        }
    }
}

impl Serialize for MaxResponseBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Disabled => serializer.serialize_str("disabled"),
            Self::Bytes(bytes) => serializer.serialize_u32(*bytes),
        }
    }
}

impl<'de> Deserialize<'de> for MaxResponseBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MaxResponseBytesVisitor;

        impl serde::de::Visitor<'_> for MaxResponseBytesVisitor {
            type Value = MaxResponseBytes;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a byte count, \"disabled\", or a boolean")
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<MaxResponseBytes, E> {
                Ok(MaxResponseBytes::from(v))
            }

            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<MaxResponseBytes, E> {
                let bytes = u32::try_from(v).map_err(|_| {
                    serde::de::Error::custom(format!("max response bytes '{v}' exceeds u32"))
                })?;

                Ok(MaxResponseBytes::from_bytes(bytes))
            }

            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<MaxResponseBytes, E> {
                let unsigned = u64::try_from(v).map_err(|_| {
                    serde::de::Error::custom(format!("max response bytes '{v}' is negative"))
                })?;
                self.visit_u64(unsigned)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<MaxResponseBytes, E> {
                v.parse().map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(MaxResponseBytesVisitor)
    }
}

impl schematic::Schematic for MaxResponseBytes {
    fn schema_name() -> Option<String> {
        Some("MaxResponseBytes".to_owned())
    }

    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        use schematic::schema::{BooleanType, EnumType, IntegerType, LiteralValue, UnionType};

        schema.union(UnionType::new_any([
            schema
                .nest()
                .integer(IntegerType::new_kind(schematic::schema::IntegerKind::U32)),
            schema.nest().enumerable(EnumType::new([
                LiteralValue::String("disabled".into()),
                LiteralValue::String("off".into()),
            ])),
            schema.nest().boolean(BooleanType::default()),
        ]))
    }
}

/// Controls whether the provider should apply prompt caching.
///
/// Providers map these values to their native caching mechanisms:
///
/// - Anthropic: `cache_control` annotations and automatic caching
/// - Other providers: provider-specific caching hints
///
/// When `Off`, the provider skips all caching annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CachePolicy {
    /// No caching.
    /// The provider skips all cache annotations.
    Off,

    /// Standard caching with provider-default TTL (typically ~5 minutes).
    #[default]
    Short,

    /// Extended caching with longer TTL (typically ~1 hour where supported).
    Long,

    /// Custom duration.
    /// Not all providers support arbitrary durations; unsupported values are
    /// rounded to the nearest available option.
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
