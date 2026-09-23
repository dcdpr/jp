use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_anthropic::{
    Client,
    errors::{AnthropicError, ApiError},
    types::{CreateMessagesRequest, CreateMessagesRequestBuilder, MessageBuilder, MessageRole},
};
use backon::ExponentialBuilder;
use httpmock::{HttpMockRequest, HttpMockResponse, MockServer, prelude::POST};
use serde_json::{Value, json};

fn client_for(server: &MockServer) -> Client {
    Client::builder()
        .api_key("test_secret")
        .base_url(server.base_url())
        .build()
        .unwrap()
}

/// A backoff short enough that a retrying test finishes quickly.
fn fast_backoff() -> ExponentialBuilder {
    ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(10))
        .with_factor(2.0)
        .with_max_delay(Duration::from_millis(100))
}

fn hello_request() -> CreateMessagesRequest {
    CreateMessagesRequestBuilder::default()
        .model("test-model".to_string())
        .messages(vec![
            MessageBuilder::default()
                .role(MessageRole::User)
                .content("Hello world!")
                .build()
                .unwrap(),
        ])
        .build()
        .unwrap()
}

fn json_response(status: u16, body: &Value) -> HttpMockResponse {
    HttpMockResponse::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body.to_string())
        .build()
}

/// Answer the first `failures` requests with `failure`, and every later one
/// with a successful message.
///
/// Returns a counter of the requests the responder saw.
fn fail_then_succeed(
    failures: usize,
    failure: HttpMockResponse,
) -> (
    impl Fn(&HttpMockRequest) -> HttpMockResponse + Send + Sync + 'static,
    Arc<AtomicUsize>,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let responder = {
        let calls = Arc::clone(&calls);
        move |_: &HttpMockRequest| {
            if calls.fetch_add(1, Ordering::SeqCst) < failures {
                return failure.clone();
            }

            json_response(
                200,
                &json!({ "content": [{"type": "text", "text": "retried response"}] }),
            )
        }
    };

    (responder, calls)
}

#[tokio::test]
async fn test_client_build_request() {
    let request = Client::builder().api_key("test_secret").build();

    assert!(request.is_ok());
}

#[test_log::test(tokio::test)]
async fn test_successful_request_execution() {
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(200).json_body(json!({
                "content": [{"type": "text", "text": "mocked response"}]
            }));
        })
        .await;

    let result = client_for(&server)
        .messages()
        .create(hello_request())
        .await
        .unwrap();

    mock.assert_calls_async(1).await;
    assert_eq!(
        result.content[0].as_text().map(|t| t.text.as_str()),
        Some("mocked response")
    );
}

/// Throttling that never lets up is retried until the backoff is exhausted, and
/// then surfaces as a rate limit.
#[tokio::test]
async fn test_with_backoff_basic() {
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(429).body("Too Many Requests");
        })
        .await;

    let result = client_for(&server)
        .with_backoff(fast_backoff())
        .messages()
        .create(hello_request())
        .await;

    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            AnthropicError::RateLimit { .. }
        ),
        "actual: {:?}",
        &result
    );

    // The first attempt plus the backoff's three retries.
    mock.assert_calls_async(4).await;
}

#[tokio::test]
async fn test_default_backoff_retries() {
    let server = MockServer::start_async().await;

    let throttled = HttpMockResponse::builder()
        .status(429)
        .body("Too Many Requests")
        .build();
    let (responder, calls) = fail_then_succeed(3, throttled);

    server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages");
            then.respond_with(responder);
        })
        .await;

    let result = client_for(&server)
        .with_backoff(fast_backoff())
        .messages()
        .create(hello_request())
        .await
        .expect("the request succeeds once throttling stops");

    // Three throttled attempts, then the one that succeeded.
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        result.content[0].as_text().map(|t| t.text.as_str()),
        Some("retried response")
    );
}

/// An overloaded server is retried in place, even when the response carries
/// quota headers that would otherwise mark a spent window.
///
/// A quota rejection is never retried, so a `529` misread as one would end the
/// request after the first call.
#[tokio::test]
async fn test_overloaded_with_quota_headers_is_retried() {
    let server = MockServer::start_async().await;

    let overloaded = HttpMockResponse::builder()
        .status(529)
        .header(
            "anthropic-ratelimit-unified-representative-claim",
            "five_hour",
        )
        .header("anthropic-ratelimit-unified-overage-status", "rejected")
        .header("content-type", "application/json")
        .body(
            json!({
                "type": "error",
                "error": { "type": "overloaded_error", "message": "Overloaded" }
            })
            .to_string(),
        )
        .build();
    let (responder, calls) = fail_then_succeed(1, overloaded);

    server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages");
            then.respond_with(responder);
        })
        .await;

    let result = client_for(&server)
        .with_backoff(fast_backoff())
        .messages()
        .create(hello_request())
        .await
        .expect("the overloaded response is retried");

    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        result.content[0].as_text().map(|t| t.text.as_str()),
        Some("retried response")
    );
}

#[tokio::test]
async fn test_error_handling_bad_request() {
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(400).json_body(json!({
                "type": "error",
                "error": {
                    "type": "invalid_request_error",
                    "message": "Bad request"
                }
            }));
        })
        .await;

    let result = client_for(&server).messages().create(hello_request()).await;

    mock.assert_calls_async(1).await;
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            AnthropicError::Api(ApiError { error_type, .. }) if error_type == "invalid_request_error"
        ),
        "actual: {:?}",
        &result
    );
}

#[tokio::test]
async fn test_error_handling_unauthorized() {
    let server = MockServer::start_async().await;

    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages");
            then.status(401).json_body(json!({
                "type": "error",
                "error": {
                    "type": "authentication_error",
                    "message": "Unauthorized"
                }
            }));
        })
        .await;

    let result = client_for(&server).messages().create(hello_request()).await;

    mock.assert_calls_async(1).await;
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            AnthropicError::Auth {
                status: 401,
                error: ApiError { error_type, .. },
            } if error_type == "authentication_error"
        ),
        "actual: {:?}",
        &result
    );
}
