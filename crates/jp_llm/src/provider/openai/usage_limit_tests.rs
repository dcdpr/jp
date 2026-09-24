//! A spent subscription is recognized from its typed body, with or without the
//! usage headers.

use chrono::DateTime;
use openai_responses::StreamError as OpenaiStreamError;
use reqwest::{
    StatusCode,
    header::{HeaderMap, HeaderName, HeaderValue},
};

use super::map_error;
use crate::StreamErrorKind;

/// The body the subscription host answers a spent plan with.
const USAGE_LIMIT_REACHED: &str =
    r#"{"error":{"type":"usage_limit_reached","plan_type":"pro","resets_at":1789571252}}"#;

fn status_429(body: &str, headers: HeaderMap) -> OpenaiStreamError {
    OpenaiStreamError::Status {
        status: StatusCode::TOO_MANY_REQUESTS,
        headers: Box::new(headers),
        body: body.to_owned(),
    }
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    pairs
        .iter()
        .map(|(name, value)| {
            (
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            )
        })
        .collect()
}

/// No usage headers: the body alone says the plan is spent, and when it
/// reopens.
#[test]
fn test_a_typed_usage_limit_is_exhaustion_without_headers() {
    let error = map_error(
        status_429(USAGE_LIMIT_REACHED, HeaderMap::new()),
        "gpt-5.6-luna",
    );

    assert_eq!(
        error.kind,
        StreamErrorKind::SubscriptionExhausted,
        "{error}"
    );
    assert_eq!(
        error.quota_reset,
        DateTime::from_timestamp(1_789_571_252, 0)
    );
    assert_eq!(error.quota_scope, None);
}

/// With the headers, they still name the spent window's scope.
#[test]
fn test_the_headers_still_scope_a_typed_usage_limit() {
    let error = map_error(
        status_429(
            USAGE_LIMIT_REACHED,
            headers(&[
                ("x-codex-primary-used-percent", "100"),
                ("x-codex-primary-window-minutes", "10080"),
            ]),
        ),
        "gpt-5.6-luna",
    );

    assert_eq!(
        error.kind,
        StreamErrorKind::SubscriptionExhausted,
        "{error}"
    );
    assert_eq!(error.quota_scope.as_deref(), Some("account"));
}

/// An ordinary rate limit stays retryable in place.
#[test]
fn test_a_plain_rate_limit_stays_a_rate_limit() {
    let error = map_error(
        status_429(
            r#"{"error":{"type":"rate_limit_exceeded","message":"slow down"}}"#,
            HeaderMap::new(),
        ),
        "gpt-5.6-luna",
    );

    assert_eq!(error.kind, StreamErrorKind::RateLimit, "{error}");
}
