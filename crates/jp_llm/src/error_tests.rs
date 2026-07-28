use std::io;

use super::*;

/// A `reqwest_eventsource::Error::InvalidStatusCode` carrying `status` and
/// `body`, as the provider stream paths receive it.
fn invalid_status(status: u16, body: &str) -> reqwest_eventsource::Error {
    let response = http::Response::builder()
        .status(status)
        .body(body.to_owned())
        .expect("valid response");

    reqwest_eventsource::Error::InvalidStatusCode(
        reqwest::StatusCode::from_u16(status).expect("valid status"),
        reqwest::Response::from(response),
    )
}

#[tokio::test]
async fn stream_ended_classifies_as_retryable() {
    // A stream that ends without a terminal event is the disconnect case (e.g.
    // the socket dropped mid-response). It must route through the retry layer,
    // not be surfaced as a fatal error.
    let err = StreamError::from_eventsource(reqwest_eventsource::Error::StreamEnded).await;
    assert!(err.is_retryable());
}

#[tokio::test]
async fn oversized_prompt_body_is_a_context_window_error() {
    let err = StreamError::from_eventsource(invalid_status(
        400,
        r#"{"error":{"message":"prompt is too long: 531500 tokens > 200000 maximum"}}"#,
    ))
    .await;

    assert_eq!(err.kind, StreamErrorKind::ContextWindowExceeded);
    assert!(!err.is_retryable());
}

/// A plain 429 is a rate limit, and its `Retry-After` survives.
#[tokio::test]
async fn a_429_is_a_rate_limit() {
    let err = StreamError::from_eventsource(invalid_status(429, "slow down")).await;

    assert_eq!(err.kind, StreamErrorKind::RateLimit);
    assert!(err.is_retryable());
}

/// A request timeout and a conflict are retryable without being rate limits.
#[tokio::test]
async fn timeout_and_conflict_are_transient() {
    for code in [408, 409] {
        let err = StreamError::from_eventsource(invalid_status(code, "try again")).await;

        assert_eq!(err.kind, StreamErrorKind::Transient, "HTTP {code}");
        assert!(err.is_retryable(), "HTTP {code}");
    }
}

/// A 400 carries no retry signal and no recognized phrasing, so it stays fatal.
#[tokio::test]
async fn a_plain_400_is_not_retryable() {
    let err = StreamError::from_eventsource(invalid_status(400, "malformed request")).await;

    assert_eq!(err.kind, StreamErrorKind::Other);
    assert!(!err.is_retryable());
}

/// A 429 whose body reads like a window overflow must stay a retryable rate
/// limit.
///
/// The status code is an authoritative rate-limit signal; the body heuristic is
/// a fallback for the 4xx responses that carry no such signal.
/// Letting the text win would turn a wait-and-retry into a fatal error and
/// discard the retry timing with it.
#[tokio::test]
async fn a_429_outranks_a_context_window_phrasing_in_the_body() {
    let err = StreamError::from_eventsource(invalid_status(
        429,
        r#"{"error":{"message":"Rate limit reached: too many tokens per minute."}}"#,
    ))
    .await;

    assert_eq!(err.kind, StreamErrorKind::RateLimit);
    assert!(err.is_retryable());
}

#[test]
fn extract_retry_after_from_retry_after_ms() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after-ms", "1500".parse().unwrap());

    assert_eq!(
        extract_retry_after(&headers),
        Some(Duration::from_millis(1500))
    );
}

#[test]
fn extract_retry_after_ms_takes_priority_over_retry_after() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after-ms", "500".parse().unwrap());
    headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());

    // retry-after-ms is more precise, should be preferred.
    assert_eq!(
        extract_retry_after(&headers),
        Some(Duration::from_millis(500))
    );
}

#[test]
fn extract_retry_after_from_standard_header() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());

    assert_eq!(extract_retry_after(&headers), Some(Duration::from_secs(30)));
}

#[test]
fn extract_retry_after_accepts_float_seconds() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "1.5".parse().unwrap());

    assert_eq!(
        extract_retry_after(&headers),
        Some(Duration::from_millis(1500))
    );
}

#[test]
fn extract_retry_after_ignores_http_date() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::RETRY_AFTER,
        "Wed, 21 Oct 2025 07:28:00 GMT".parse().unwrap(),
    );

    // HTTP-date is not supported, should return None.
    assert_eq!(extract_retry_after(&headers), None);
}

#[test]
fn extract_retry_after_from_ietf_ratelimit_header() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("ratelimit", "remaining=0; t=45".parse().unwrap());

    assert_eq!(extract_retry_after(&headers), Some(Duration::from_secs(45)));
}

#[test]
fn extract_retry_after_from_openai_reset_requests() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-reset-requests", "6m0s".parse().unwrap());

    assert_eq!(extract_retry_after(&headers), Some(Duration::from_mins(6)));
}

#[test]
fn extract_retry_after_from_openai_reset_tokens() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-reset-tokens", "1s".parse().unwrap());

    assert_eq!(extract_retry_after(&headers), Some(Duration::from_secs(1)));
}

#[test]
fn extract_retry_after_openai_takes_max_of_both() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-reset-requests", "2s".parse().unwrap());
    headers.insert("x-ratelimit-reset-tokens", "6m0s".parse().unwrap());

    assert_eq!(extract_retry_after(&headers), Some(Duration::from_mins(6)));
}

#[test]
fn extract_retry_after_from_ratelimit_reset() {
    let future_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 45;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-reset", future_ts.to_string().parse().unwrap());

    let result = extract_retry_after(&headers).unwrap();
    // Allow 1s tolerance for test execution time.
    assert!(result.as_secs() >= 44 && result.as_secs() <= 46);
}

#[test]
fn extract_retry_after_prefers_standard_header() {
    let future_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 120;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(reqwest::header::RETRY_AFTER, "10".parse().unwrap());
    headers.insert("x-ratelimit-reset", future_ts.to_string().parse().unwrap());

    assert_eq!(extract_retry_after(&headers), Some(Duration::from_secs(10)));
}

#[test]
fn extract_retry_after_past_reset_returns_none() {
    let past_ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 60;

    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-ratelimit-reset", past_ts.to_string().parse().unwrap());

    assert_eq!(extract_retry_after(&headers), None);
}

#[test]
fn extract_retry_after_empty_headers() {
    let headers = reqwest::header::HeaderMap::new();
    assert_eq!(extract_retry_after(&headers), None);
}

#[test]
fn human_duration_seconds() {
    assert_eq!(parse_human_duration("1s"), Some(1));
    assert_eq!(parse_human_duration("30s"), Some(30));
}

#[test]
fn human_duration_minutes_and_seconds() {
    assert_eq!(parse_human_duration("6m0s"), Some(360));
    assert_eq!(parse_human_duration("1m30s"), Some(90));
}

#[test]
fn human_duration_hours() {
    assert_eq!(parse_human_duration("1h30m0s"), Some(5400));
    assert_eq!(parse_human_duration("2h"), Some(7200));
}

#[test]
fn human_duration_milliseconds_rounds_up() {
    assert_eq!(parse_human_duration("200ms"), Some(1));
    assert_eq!(parse_human_duration("0ms"), None);
}

#[test]
fn human_duration_mixed_with_ms() {
    // 1 second + 500ms = 1s (ms doesn't add to whole seconds).
    assert_eq!(parse_human_duration("1s500ms"), Some(1));
}

#[test]
fn human_duration_zero_returns_none() {
    assert_eq!(parse_human_duration("0s"), None);
    assert_eq!(parse_human_duration("0m0s"), None);
}

#[test]
fn human_duration_invalid() {
    assert_eq!(parse_human_duration(""), None);
    assert_eq!(parse_human_duration("abc"), None);
    assert_eq!(parse_human_duration("5x"), None);
}

#[test]
fn text_retry_after_n_seconds() {
    let text = "Rate limit exceeded. Please retry after 30 seconds.";
    assert_eq!(extract_retry_from_text(text), Some(Duration::from_secs(30)));
}

#[test]
fn text_wait_n_seconds() {
    let text = "Too many requests. Please wait 60 seconds before trying again.";
    assert_eq!(extract_retry_from_text(text), Some(Duration::from_mins(1)));
}

#[test]
fn text_try_again_in_ns() {
    let text = "Service busy, try again in 5s";
    assert_eq!(extract_retry_from_text(text), Some(Duration::from_secs(5)));
}

#[test]
fn text_try_again_in_float() {
    let text = "Overloaded, try again in 5.5s please";
    assert_eq!(
        extract_retry_from_text(text),
        Some(Duration::from_secs(6)) // ceil(5.5)
    );
}

#[test]
fn text_try_again_in_float_with_trailing_period() {
    // Matches the exact shape of OpenAI's in-stream rate-limit messages:
    // "...Please try again in 2.398s. Visit ..."
    let text = "Rate limit reached on tokens per min (TPM): Limit 2000000, \
                Used 1354056, Requested 725891. Please try again in 2.398s. \
                Visit https://platform.openai.com/account/rate-limits to learn more.";
    assert_eq!(
        extract_retry_from_text(text),
        Some(Duration::from_secs(3)) // ceil(2.398)
    );
}

#[test]
fn text_retry_after_colon() {
    let text = "Error: retry-after: 15";
    assert_eq!(extract_retry_from_text(text), Some(Duration::from_secs(15)));
}

#[test]
fn text_gemini_retry_delay() {
    let text = r#"{"error":{"details":[{"retryDelay":"30s"}]}}"#;
    assert_eq!(extract_retry_from_text(text), Some(Duration::from_secs(30)));
}

#[test]
fn text_no_pattern_returns_none() {
    assert_eq!(extract_retry_from_text("Something went wrong"), None);
    assert_eq!(extract_retry_from_text(""), None);
}

#[test]
fn transient_network_error_matches_proxy_upstream_failures() {
    // Proxy error bodies that don't match any provider's error envelope, so
    // they surface as `Other` via the generic fallback classification.
    assert!(looks_like_transient_network_error(
        r#"{"error":"the upstream unreachable"}"#
    ));
    assert!(looks_like_transient_network_error("no healthy upstream"));
    assert!(looks_like_transient_network_error(
        "upstream connect error or disconnect/reset before headers"
    ));
    assert!(looks_like_transient_network_error("Network is unreachable"));
    assert!(looks_like_transient_network_error("host unreachable"));
}

#[test]
fn transient_network_error_ignores_unrelated_errors() {
    assert!(!looks_like_transient_network_error("invalid request"));
    assert!(!looks_like_transient_network_error(
        "model not found: claude-nonexistent"
    ));
    assert!(!looks_like_transient_network_error(""));
}

#[test]
fn other_error_with_upstream_failure_message_is_retryable() {
    // The exact shape of the reported failure: a proxy body wrapped by
    // `AnthropicError::Unknown`, classified as `Other` by the catch-all arm.
    let error = StreamError::other(r#"unknown error: {"error":"the upstream unreachable"}"#);
    assert!(error.is_retryable());
}

#[test]
fn other_error_without_transient_pattern_is_not_retryable() {
    let error = StreamError::other("unknown error: something exploded");
    assert!(!error.is_retryable());
}

#[test]
fn context_window_error_detects_provider_messages() {
    // The exact shape of the reported Anthropic failure.
    assert!(looks_like_context_window_error(
        "api error: invalid_request_error: prompt is too long: 531500 tokens > 200000 maximum"
    ));
    assert!(looks_like_context_window_error("context_length_exceeded"));
    assert!(looks_like_context_window_error(
        "This model's maximum context length is 8192 tokens. However, your messages resulted in \
         10000 tokens."
    ));
    assert!(looks_like_context_window_error(
        "The input token count (1200000) exceeds the maximum number of tokens allowed (1048576)."
    ));
    assert!(looks_like_context_window_error(
        "the request exceeds the available context size, try increasing it"
    ));
}

#[test]
fn context_window_error_ignores_unrelated_errors() {
    assert!(!looks_like_context_window_error("invalid request"));
    assert!(!looks_like_context_window_error(
        "api error: rate_limit_error: too many requests"
    ));
    assert!(!looks_like_context_window_error(""));
}

#[test]
fn context_window_error_is_not_retryable() {
    // Retrying sends the same oversized prompt; the request has to shrink.
    let error = StreamError::context_window_exceeded("prompt is too long");
    assert!(!error.is_retryable());
    assert_eq!(error.kind, StreamErrorKind::ContextWindowExceeded);
}

#[test]
fn display_does_not_repeat_a_source_already_in_the_message() {
    // Providers build the message from the source's own rendering; the source
    // must not then be appended a second time.
    let source = io::Error::other("prompt is too long: 531500 tokens > 200000 maximum");
    let error = StreamError::other(source.to_string()).with_source(source);

    assert_eq!(
        error.to_string(),
        "prompt is too long: 531500 tokens > 200000 maximum"
    );
}

#[test]
fn display_appends_a_source_that_adds_information() {
    let source = io::Error::other("connection reset by peer");
    let error = StreamError::other("request failed").with_source(source);

    assert_eq!(
        error.to_string(),
        "request failed: connection reset by peer"
    );
}
