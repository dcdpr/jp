use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AnthropicError {
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("api error: {0}")]
    Api(#[from] ApiError),

    /// The request was rejected with `429`.
    ///
    /// `limits` carries Anthropic's unified rate-limit headers, which
    /// distinguish a quota rejection from ordinary throttling; see
    /// [`UnifiedRateLimit`].
    #[error("rate limited (retry after {} seconds)", retry_after.unwrap_or_default())]
    RateLimit {
        retry_after: Option<u64>,
        limits: UnifiedRateLimit,
    },

    /// The request was rejected with `401` or `403`.
    ///
    /// Kept separate from [`Api`] because the status is what makes it
    /// actionable: the credential itself was refused, so the caller
    /// re-authenticates rather than retries.
    ///
    /// [`Api`]: Self::Api
    #[error("authentication rejected ({status}): {error}")]
    Auth { status: u16, error: ApiError },

    #[error("failed to deserialize response: {0}")]
    Deserialization(#[from] serde_json::Error),

    #[error("stream transport error: {0}")]
    StreamTransport(String),

    #[error("unknown error: {0}")]
    Unknown(String),
}

/// Anthropic's unified rate-limit headers.
///
/// A subscription account's usage windows are reported here rather than in the
/// error body: the same `429 rate_limit_error` covers a spent usage window,
/// ordinary capacity throttling, and a rejected request, so only these headers
/// tell them apart.
///
/// See:
/// <https://support.claude.com/en/articles/11145838-using-claude-code-with-your-pro-or-max-plan>
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnifiedRateLimit {
    /// `allowed`, `allowed_warning`, or `rejected`.
    pub status: Option<String>,

    /// The window that is exhausted: `five_hour`, `seven_day`,
    /// `seven_day_opus`, `seven_day_sonnet`, or `overage` (the paid extra-usage
    /// allowance).
    ///
    /// `five_hour`, `seven_day`, and `overage` cover the whole account; the
    /// others cover one model family.
    pub representative_claim: Option<String>,

    /// When the exhausted window resets, as Unix seconds.
    pub reset: Option<u64>,

    /// Whether paid spillover ("extra usage") can serve the request: `allowed`,
    /// `allowed_warning`, or `rejected`.
    pub overage_status: Option<String>,

    /// When the paid spillover allowance resets, as Unix seconds.
    pub overage_reset: Option<u64>,

    /// Why paid spillover could not serve the request, when it could not.
    ///
    /// Reported by the unified limiter, e.g. `out_of_credits`,
    /// `org_level_disabled`, `seat_tier_zero_credit_limit`.
    /// Absent when spillover is available.
    pub overage_disabled_reason: Option<String>,

    /// Per-window utilization, as a fraction of the allowance consumed.
    ///
    /// Reported on every response, not only on a rejection.
    pub windows: Vec<WindowUtilization>,
}

/// How much of one usage window has been consumed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowUtilization {
    /// The window this describes: `five_hour` or `seven_day`.
    pub claim: String,

    /// The fraction of the window's allowance consumed, from 0 to 1.
    pub utilization: Option<f64>,

    /// When this window resets, as Unix seconds.
    pub reset: Option<u64>,

    /// The warning threshold this window has crossed, when it has crossed one.
    ///
    /// Presence is the signal: the provider only sends it once usage passes a
    /// threshold it wants the client to surface.
    pub surpassed_threshold: Option<f64>,
}

impl UnifiedRateLimit {
    /// Read the unified headers from a response.
    #[must_use]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let string = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let number = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
        };

        let float = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
        };

        // The per-window headers are abbreviated (`5h`, `7d`) while the
        // window names elsewhere are spelled out; normalize to the spelled-out
        // form so one vocabulary reaches callers.
        let windows = [("5h", "five_hour"), ("7d", "seven_day")]
            .into_iter()
            .map(|(abbrev, claim)| WindowUtilization {
                claim: claim.to_owned(),
                utilization: float(&format!("anthropic-ratelimit-unified-{abbrev}-utilization")),
                reset: number(&format!("anthropic-ratelimit-unified-{abbrev}-reset")),
                surpassed_threshold: float(&format!(
                    "anthropic-ratelimit-unified-{abbrev}-surpassed-threshold"
                )),
            })
            .filter(|window| window.utilization.is_some() || window.reset.is_some())
            .collect();

        Self {
            status: string("anthropic-ratelimit-unified-status"),
            representative_claim: string("anthropic-ratelimit-unified-representative-claim"),
            reset: number("anthropic-ratelimit-unified-reset"),
            overage_status: string("anthropic-ratelimit-unified-overage-status"),
            overage_reset: number("anthropic-ratelimit-unified-overage-reset"),
            overage_disabled_reason: string("anthropic-ratelimit-unified-overage-disabled-reason"),
            windows,
        }
    }

    /// Whether the subscription allowance is spent.
    ///
    /// A response can report this while still succeeding: paid spillover keeps
    /// serving requests past the window when the account has extra usage
    /// enabled, so a `200` carrying this state means the request was billed
    /// rather than covered by the allowance.
    #[must_use]
    pub fn is_rejected(&self) -> bool {
        self.status.as_deref() == Some("rejected")
    }

    /// The window that has crossed a warning threshold, if any.
    #[must_use]
    pub fn warning(&self) -> Option<&WindowUtilization> {
        self.windows
            .iter()
            .find(|window| window.surpassed_threshold.is_some())
    }

    /// Whether this rejection is a quota limit rather than throttling.
    ///
    /// A rejection that names the exhausted window, or reports on paid
    /// spillover, is about an allowance being spent.
    /// One that carries neither is not a quota limit at all, however much its
    /// status code resembles one.
    ///
    /// A status that explicitly reports the allowance as still available
    /// (`allowed`, `allowed_warning`) overrides both: the request was refused
    /// for another reason.
    /// A missing status does not, since a quota rejection is not known to
    /// always carry one.
    #[must_use]
    pub fn is_quota_rejection(&self) -> bool {
        let allowance_available =
            matches!(self.status.as_deref(), Some("allowed" | "allowed_warning"));

        !allowance_available
            && (self.representative_claim.is_some() || self.overage_status.is_some())
    }

    /// Whether none of the unified headers were present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The wire-format envelope for Anthropic API errors.
///
/// ```json
/// {
///     "type": "error",
///     "error": {
///         "type": "overloaded_error",
///         "message": "Overloaded"
///     }
/// }
/// ```
///
/// The top-level `type` is always `"error"` and is discarded during
/// deserialization.
/// Only the inner [`ApiError`] is kept.
#[derive(Debug, Deserialize)]
pub(crate) struct ApiErrorEnvelope {
    pub error: ApiError,
}

/// An error returned by the Anthropic API.
///
/// Represents the inner `error` object from the standard error envelope.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq, Serialize)]
pub struct ApiError {
    /// The error type, e.g. `"overloaded_error"`, `"rate_limit_error"`,
    /// `"invalid_request_error"`.
    #[serde(rename = "type")]
    pub error_type: String,

    /// Human-readable error message.
    pub message: Option<String>,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: {}",
            self.error_type,
            self.message.as_deref().unwrap_or("(no message)")
        )
    }
}

impl std::error::Error for ApiError {}

pub(crate) fn map_deserialization_error(e: serde_json::Error, _bytes: &[u8]) -> AnthropicError {
    AnthropicError::Deserialization(e)
}

#[cfg(test)]
#[path = "errors_tests.rs"]
mod tests;
