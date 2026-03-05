use super::*;

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
