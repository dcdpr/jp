use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_anthropic::errors::AnthropicError;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use serde_json::Value;

use crate::stream::aggregator::tool_call_request::AggregationError;

pub(crate) type Result<T> = std::result::Result<T, Error>;

/// A provider-agnostic streaming error.
#[derive(Debug)]
pub struct StreamError {
    /// The kind of streaming error.
    pub kind: StreamErrorKind,

    /// Whether and when the request can be retried.
    ///
    /// If `Some`, the request can be retried after the specified duration.
    /// If `None`, the caller should use exponential backoff or not retry.
    pub retry_after: Option<Duration>,

    /// Human-readable error message.
    message: String,

    /// The underlying source of the error.
    ///
    /// This is kept for logging and display purposes, but callers should
    /// make decisions based on `kind` and `retry_after`, not the source.
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl StreamError {
    /// Create a new stream error.
    #[must_use]
    pub fn new(kind: StreamErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
            source: None,
        }
    }

    /// Create a timeout error.
    #[must_use]
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(StreamErrorKind::Timeout, message)
    }

    /// Create a connection error.
    #[must_use]
    pub fn connect(message: impl Into<String>) -> Self {
        Self::new(StreamErrorKind::Connect, message)
    }

    /// Create a rate limit error.
    #[must_use]
    pub fn rate_limit(retry_after: Option<Duration>) -> Self {
        Self {
            kind: StreamErrorKind::RateLimit,
            retry_after,
            message: "Rate limited".into(),
            source: None,
        }
    }

    /// Create a transient error.
    #[must_use]
    pub fn transient(message: impl Into<String>) -> Self {
        Self::new(StreamErrorKind::Transient, message)
    }

    /// Create an error from another error type.
    #[must_use]
    pub fn other(message: impl Into<String>) -> Self {
        Self::new(StreamErrorKind::Other, message)
    }

    /// Set the `retry_after` duration.
    #[must_use]
    pub fn with_retry_after(mut self, duration: Duration) -> Self {
        self.retry_after = Some(duration);
        self
    }

    /// Set the source error.
    #[must_use]
    pub fn with_source(
        mut self,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Returns whether this error is likely retryable.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            StreamErrorKind::Timeout
                | StreamErrorKind::Connect
                | StreamErrorKind::RateLimit
                | StreamErrorKind::Transient
        ) || self.retry_after.is_some()
    }
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(ref source) = self.source {
            write!(f, ": {source}")?;
        }

        Ok(())
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// Canonical classifier for [`reqwest::Error`].
///
/// Provider-specific `From` impls should delegate to this when they encounter
/// an inner [`reqwest::Error`], rather than re-implementing the classification
/// logic.
impl From<reqwest::Error> for StreamError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            StreamError::timeout(err.to_string()).with_source(err)
        } else if err.is_connect() {
            StreamError::connect(err.to_string()).with_source(err)
        } else if err.status().is_some_and(|s| s == 429) {
            StreamError::rate_limit(None).with_source(err)
        } else if err
            .status()
            .is_some_and(|s| matches!(s.as_u16(), 408 | 409 | _ if s.as_u16() >= 500))
            || err.is_body()
            || err.is_decode()
        {
            StreamError::transient(err.to_string()).with_source(err)
        } else {
            StreamError::other(err.to_string()).with_source(err)
        }
    }
}

/// Canonical classifier for [`reqwest_eventsource::Error`].
///
/// Delegates the [`Transport`] variant to the [`reqwest::Error`] classifier.
/// Classifies [`InvalidStatusCode`] by its HTTP status, extracts `Retry-After`
/// headers, and honours the non-standard `x-should-retry` header used by
/// OpenAI.
///
/// Provider-specific `From` impls should delegate to this when they encounter
/// an inner [`reqwest_eventsource::Error`], rather than re-implementing the
/// classification logic.
///
/// [`Transport`]: reqwest_eventsource::Error::Transport
/// [`InvalidStatusCode`]: reqwest_eventsource::Error::InvalidStatusCode
impl From<reqwest_eventsource::Error> for StreamError {
    fn from(err: reqwest_eventsource::Error) -> Self {
        use reqwest_eventsource::Error;

        match err {
            Error::Transport(error) => Self::from(error),
            Error::InvalidStatusCode(status, response) => {
                let headers = response.headers();
                let retry_after = extract_retry_after(headers);
                let code = status.as_u16();

                // Non-standard `x-should-retry` header overrides
                // status-code heuristics.
                let retryable = match header_str(headers, "x-should-retry") {
                    Some("true") => true,
                    Some("false") => false,
                    _ => matches!(code, 408 | 409 | 429 | _ if code >= 500),
                };

                if !retryable {
                    StreamError::other(format!("HTTP {status}"))
                } else if code == 429 {
                    StreamError::rate_limit(retry_after)
                } else {
                    let err = StreamError::transient(format!("HTTP {status}"));
                    match retry_after {
                        Some(d) => err.with_retry_after(d),
                        None => err,
                    }
                }
            }
            error @ (Error::Utf8(_)
            | Error::Parser(_)
            | Error::InvalidContentType(_, _)
            | Error::InvalidLastEventId(_)
            | Error::StreamEnded) => StreamError::other(error.to_string()).with_source(error),
        }
    }
}

/// The kind of streaming error.
///
/// This abstraction allows the resilience layer to make retry decisions without
/// knowing the specific provider implementation details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamErrorKind {
    /// Request timed out.
    Timeout,

    /// Failed to establish connection.
    Connect,

    /// Rate limited by the provider.
    RateLimit,

    /// Transient error (server error, temporary failure).
    /// These are typically safe to retry.
    Transient,

    /// The API key's quota has been exhausted.
    /// This is not retryable — the user needs to top up or change plans.
    InsufficientQuota,

    /// Other errors that are not categorized.
    /// These may or may not be retryable depending on the specific error.
    Other,
}

impl StreamErrorKind {
    /// Returns the error kind as a string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Timeout => "Timeout",
            Self::Connect => "Connection error",
            Self::RateLimit => "Rate limited",
            Self::Transient => "Server error",
            Self::InsufficientQuota => "Insufficient API quota",
            Self::Other => "Stream Error",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Streaming error with provider-agnostic classification.
    #[error(transparent)]
    Stream(#[from] StreamError),

    #[error("OpenRouter error: {0}")]
    OpenRouter(#[from] jp_openrouter::Error),

    #[error("Conversation error: {0}")]
    Conversation(#[from] jp_conversation::Error),

    #[error("Config error: {0}")]
    Config(#[from] jp_config::Error),

    #[error("Missing environment variable: {0}")]
    MissingEnv(String),

    #[error("Invalid URL: {0}")]
    Url(#[from] url::ParseError),

    #[error("Invalid response received: {0}")]
    InvalidResponse(String),

    #[error("OpenAI client error: {0}")]
    OpenaiClient(#[from] openai_responses::CreateError),

    #[error("OpenAI event error: {0}")]
    OpenaiEvent(Box<reqwest_eventsource::Error>),

    #[error("OpenAI response error: {0:?}")]
    OpenaiResponse(openai_responses::types::response::Error),

    #[error("OpenAI status code error: {:?} - {}", .status_code, .response)]
    OpenaiStatusCode {
        status_code: reqwest::StatusCode,
        response: String,
    },

    #[error("Gemini error: {0}")]
    Gemini(gemini_client_rs::GeminiError),

    #[error("Ollama error: {0}")]
    Ollama(#[from] ollama_rs::error::OllamaError),

    #[error("Unknown model: {0}")]
    UnknownModel(String),

    #[error("Invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Request error: {0}")]
    Request(#[from] reqwest::Error),

    #[error("Anthropic error: {0}")]
    Anthropic(#[from] AnthropicError),

    #[error("Anthropic request builder error: {0}")]
    AnthropicRequestBuilder(#[from] async_anthropic::types::CreateMessagesRequestBuilderError),

    #[error("request rate limited (retry after {} seconds)", retry_after.unwrap_or_default().as_secs())]
    RateLimit {
        retry_after: Option<std::time::Duration>,
    },

    #[error("Failed to serialize XML")]
    XmlSerialization(#[from] quick_xml::SeError),

    #[error(transparent)]
    ModelIdConfig(#[from] jp_config::model::id::ModelIdConfigError),

    #[error(transparent)]
    ModelId(#[from] jp_config::model::id::ModelIdError),

    #[error(transparent)]
    ToolCallRequestAggregator(#[from] AggregationError),
}

#[cfg(test)]
impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }

        // Good enough for testing purposes
        format!("{self:?}") == format!("{other:?}")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Tool not found: {name}")]
    NotFound { name: String },

    #[error("Tools not found: {}", names.join(", "))]
    NotFoundN { names: Vec<String> },

    #[error("Disabled in configuration")]
    Disabled,

    #[error("Command is only supported for local tools")]
    UnexpectedCommand,

    #[error("Command missing for local tool")]
    MissingCommand,

    #[error("Failed to fetch tool from MCP client")]
    McpGetToolError(#[source] jp_mcp::Error),

    #[error("Failed to run tool from MCP client")]
    McpRunToolError(#[source] jp_mcp::Error),

    #[error("Failed to serialize tool arguments")]
    SerializeArgumentsError {
        arguments: Value,
        #[source]
        error: serde_json::Error,
    },

    #[error("Tool call failed: {0}")]
    ToolCallFailed(String),

    #[error("Failed to spawn command: {command}")]
    SpawnError {
        command: String,
        #[source]
        error: std::io::Error,
    },

    #[error("Failed to open editor to edit tool call")]
    OpenEditorError {
        arguments: Value,
        #[source]
        error: open_editor::errors::OpenEditorError,
    },

    #[error("Failed to edit tool call")]
    EditArgumentsError {
        arguments: Value,
        #[source]
        error: serde_json::Error,
    },

    #[error("Template error")]
    TemplateError {
        data: String,
        #[source]
        error: minijinja::Error,
    },

    #[error("Invalid `type` property for {key}, got {value:?}, expected one of {need:?}")]
    InvalidType {
        key: String,
        value: Value,
        need: Vec<&'static str>,
    },

    #[error("Needs input: {question:?}")]
    NeedsInput { question: jp_tool::Question },

    #[error("Skipped tool execution")]
    Skipped { reason: Option<String> },

    #[error("Serialization error")]
    Serde(#[from] serde_json::Error),

    #[error("Invalid arguments (missing: {missing:?}, unknown: {unknown:?})")]
    Arguments {
        /// Required arguments that were missing.
        missing: Vec<String>,

        /// Unknown arguments that were provided.
        unknown: Vec<String>,
    },
}

impl From<jp_conversation::StreamError> for Error {
    fn from(error: jp_conversation::StreamError) -> Self {
        Self::Conversation(error.into())
    }
}

#[cfg(test)]
impl PartialEq for ToolError {
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }

        // Good enough for testing purposes
        format!("{self:?}") == format!("{other:?}")
    }
}

/// Heuristic check for quota/billing exhaustion based on error text.
///
/// This catches the common patterns across providers:
/// - OpenAI: `"insufficient_quota"`
/// - Anthropic: `"billing_error"`, `"Your credit balance is too low"`
/// - Google: `"RESOURCE_EXHAUSTED"`, `"Quota exceeded"`
/// - OpenRouter: `"insufficient credits"`, `"out of credits"`
pub(crate) fn looks_like_quota_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("insufficient_quota")
        || lower.contains("insufficient quota")
        || lower.contains("insufficient credits")
        || lower.contains("out of credits")
        || lower.contains("billing_error")
        || lower.contains("credit balance is too low")
        || lower.contains("quota exceeded")
        || lower.contains("resource_exhausted")
}

/// Extracts a retry-after duration from an error message body.
///
/// Last-resort fallback when response headers don't carry retry timing.
/// Matches common natural-language patterns found in API error responses:
///
/// - `"retry after 30 seconds"`
/// - `"retry-after: 30"`
/// - `"wait 30 seconds"`
/// - `"try again in 5s"` / `"try again in 5.5s"`
/// - `"retryDelay": "30s"` (Google Gemini JSON body)
pub(crate) fn extract_retry_from_text(text: &str) -> Option<Duration> {
    let lower = text.to_ascii_lowercase();

    // Scan for patterns and extract the first match.
    for window in lower.split_whitespace().collect::<Vec<_>>().windows(4) {
        // "retry after N second(s)"
        if window[0] == "retry"
            && window[1] == "after"
            && let Some(secs) = parse_secs_token(window[2])
        {
            return Some(Duration::from_secs(secs));
        }
        // "wait N second(s)"
        if window[0] == "wait"
            && let Some(secs) = parse_secs_token(window[1])
        {
            return Some(Duration::from_secs(secs));
        }
        // "try again in Ns" / "try again in N.Ns"
        if window[0] == "try"
            && window[1] == "again"
            && window[2] == "in"
            && let Some(secs) = parse_secs_token(window[3])
        {
            return Some(Duration::from_secs(secs));
        }
    }

    // "retry-after: N" / "retry-after:N"
    if let Some(pos) = lower.find("retry-after:") {
        let after = lower[pos + "retry-after:".len()..].trim_start();
        if let Some(secs) = after
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|s| *s > 0)
        {
            return Some(Duration::from_secs(secs));
        }
    }

    // "retryDelay": "30s" (Gemini JSON body)
    if let Some(pos) = lower.find("retrydelay") {
        let after = &lower[pos..];
        if let Some(d) = after
            .split('"')
            .find(|s| s.ends_with('s') && s[..s.len() - 1].chars().all(|c| c.is_ascii_digit()))
            .and_then(parse_human_duration)
        {
            return Some(Duration::from_secs(d));
        }
    }

    None
}

/// Extracts a retry-after duration from common rate-limit response
/// headers.
///
/// Headers are checked in decreasing order of authority:
///
/// 1. `retry-after-ms` — Non-standard (OpenAI). Millisecond precision.
/// 2. `Retry-After` — RFC 7231. Integer or float seconds (the spec
///    mandates integers, but floats are accepted). HTTP-date values are
///    not supported.
/// 3. `RateLimit` — IETF draft `t=` parameter (delta-seconds).
///    See: <https://datatracker.ietf.org/doc/draft-ietf-httpapi-ratelimit-headers>
/// 4. `x-ratelimit-reset-requests` / `x-ratelimit-reset-tokens` —
///    OpenAI-style Human-duration values (e.g. `6m0s`). Takes the longer
///    of the two if both are present.
/// 5. `x-ratelimit-reset` — Unix timestamp, converted relative to now.
fn extract_retry_after(headers: &HeaderMap) -> Option<Duration> {
    if let Some(d) = header_positive_f64(headers, "retry-after-ms")
        .map(|ms| Duration::from_secs_f64(ms / 1000.0))
    {
        return Some(d);
    }

    if let Some(d) = header_positive_f64(headers, RETRY_AFTER).map(Duration::from_secs_f64) {
        return Some(d);
    }

    // IETF draft: `RateLimit: remaining=0; t=<seconds>`
    if let Some(secs) = headers
        .get("ratelimit")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .map(str::trim)
                .find_map(|p| p.strip_prefix("t="))
        })
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
    {
        return Some(Duration::from_secs(secs));
    }

    // 4. OpenAI: `x-ratelimit-reset-requests` / `x-ratelimit-reset-tokens`
    //    Both use human-duration format (e.g. "1s", "6m0s"). Take the max.
    let requests = header_str(headers, "x-ratelimit-reset-requests").and_then(parse_human_duration);
    let tokens = header_str(headers, "x-ratelimit-reset-tokens").and_then(parse_human_duration);

    if let Some(secs) = requests.into_iter().chain(tokens).max() {
        return Some(Duration::from_secs(secs));
    }

    if let Some(reset_ts) = header_u64(headers, "x-ratelimit-reset") {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if reset_ts > now {
            return Some(Duration::from_secs(reset_ts - now));
        }
    }

    None
}

/// Read a header value as `&str`.
fn header_str(headers: &HeaderMap, name: impl reqwest::header::AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Read a header value as `u64`.
fn header_u64(headers: &HeaderMap, name: impl reqwest::header::AsHeaderName) -> Option<u64> {
    header_str(headers, name).and_then(|s| s.parse().ok())
}

/// Read a header value as a positive, finite `f64`.
fn header_positive_f64(
    headers: &HeaderMap,
    name: impl reqwest::header::AsHeaderName,
) -> Option<f64> {
    header_str(headers, name)
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| *v > 0.0 && v.is_finite())
}

/// Parses a human-style duration string into whole seconds.
///
/// Supported units: `h` (hours), `m` (minutes), `s` (seconds), `ms`
/// (milliseconds — rounded up to 1s if non-zero and total is 0).
///
/// Examples: `"1s"` → 1, `"6m0s"` → 360, `"1h30m"` → 5400,
/// `"200ms"` → 1.
///
/// Returns `None` for empty, zero, or unparseable values.
fn parse_human_duration(s: &str) -> Option<u64> {
    let mut total: u64 = 0;
    let mut has_sub_second = false;
    let mut remaining = s.trim();

    while !remaining.is_empty() {
        let num_end = remaining
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(remaining.len());

        if num_end == 0 {
            return None;
        }

        let n: u64 = remaining[..num_end].parse().ok()?;
        remaining = &remaining[num_end..];

        if remaining.starts_with("ms") {
            has_sub_second = n > 0;
            remaining = &remaining[2..];
        } else if remaining.starts_with('h') {
            total += n * 3600;
            remaining = &remaining[1..];
        } else if remaining.starts_with('m') {
            total += n * 60;
            remaining = &remaining[1..];
        } else if remaining.starts_with('s') {
            total += n;
            remaining = &remaining[1..];
        } else {
            return None;
        }
    }

    // Round sub-second durations up to 1s so we don't return 0
    // when the server asked us to wait.
    if total == 0 && has_sub_second {
        total = 1;
    }

    if total > 0 { Some(total) } else { None }
}

/// Parse a token like `"30"`, `"30s"`, `"5.5s"` into whole seconds.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_secs_token(s: &str) -> Option<u64> {
    let s = s
        .trim_end_matches('s')
        .trim_end_matches("second")
        .trim_end_matches(',');
    s.parse::<f64>()
        .ok()
        .filter(|v| *v > 0.0 && v.is_finite())
        .map(|v| v.ceil() as u64)
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod tests;
