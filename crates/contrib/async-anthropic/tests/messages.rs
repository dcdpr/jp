use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use async_anthropic::{
    Client,
    errors::{AnthropicError, ApiError},
    types::{CreateMessagesRequestBuilder, MessageBuilder, MessageContent, MessageRole},
};
use async_trait::async_trait;
use backon::ExponentialBuilder;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

// Helper trait for setting up and tearing down mock server
#[async_trait]
pub trait MockApp {
    async fn setup() -> MockServer;
}

struct TestSetup;

#[async_trait]
impl MockApp for TestSetup {
    async fn setup() -> MockServer {
        MockServer::start().await
    }
}

#[tokio::test]
async fn test_client_build_request() {
    let secret_key = "test_secret";

    let request = Client::builder().api_key(secret_key).build();

    assert!(request.is_ok());
}

#[test_log::test(tokio::test)]
async fn test_successful_request_execution() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock successful response
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "mocked response"}]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .api_key(secret_key)
        .base_url(server.uri())
        .build()
        .unwrap();

    let request = CreateMessagesRequestBuilder::default()
        .model("test-model".to_string())
        .stream(true)
        .messages(vec![
            MessageBuilder::default()
                .role(MessageRole::User)
                .content("Hello world!")
                .build()
                .unwrap(),
        ])
        .build()
        .unwrap();

    let result = client.messages().create(request).await.unwrap();

    if let MessageContent::Text(text) = &result.content[0] {
        assert_eq!(text.text, "mocked response");
    }
}

#[tokio::test]
async fn test_with_backoff_basic() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock 429 Too Many Requests, expecting retries
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string("Too Many Requests")
                .set_delay(Duration::from_millis(10)),
        )
        .up_to_n_times(20)
        .expect(1..)
        .mount(&server)
        .await;

    let backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(10))
        .with_factor(2.0)
        .with_max_delay(Duration::from_millis(100));

    let client = Client::builder()
        .base_url(server.uri())
        .api_key(secret_key)
        .build()
        .unwrap()
        .with_backoff(backoff);

    let request = CreateMessagesRequestBuilder::default()
        .model("test-model".to_string())
        .stream(true)
        .messages(vec![
            MessageBuilder::default()
                .role(MessageRole::User)
                .content("Hello world!")
                .build()
                .unwrap(),
        ])
        .build()
        .unwrap();

    let result = client.messages().create(request).await;

    assert!(result.is_err());
    assert!(
        matches!(
            result.as_ref().unwrap_err(),
            AnthropicError::RateLimit { .. }
        ),
        "actual: {:?}",
        &result
    );
}

pub struct RetryResponder {
    num_calls_before_success: Arc<Mutex<u64>>,
    calls_made: Arc<Mutex<u64>>,
}

impl RetryResponder {
    #[must_use]
    pub fn new(num_calls_before_success: u64) -> Self {
        Self {
            num_calls_before_success: Arc::new(Mutex::new(num_calls_before_success)),
            calls_made: Arc::new(Mutex::new(0)),
        }
    }
}

impl wiremock::Respond for RetryResponder {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let i = *self.calls_made.lock().unwrap();
        let succ_calls = *self.num_calls_before_success.lock().unwrap();

        if i < succ_calls {
            *self.calls_made.lock().unwrap() += 1;
            ResponseTemplate::new(429)
                .set_body_string("Too Many Requests")
                .set_delay(Duration::from_millis(10))
        } else {
            ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "retried response"}]
            }))
        }
    }
}

#[tokio::test]
async fn test_default_backoff_retries() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    let bad_calls = 3;
    let expected_calls = bad_calls + 1; // + 1 good call at the end

    let rr = RetryResponder::new(bad_calls);

    // Mock 429 Too Many Requests, expecting retries
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(rr)
        .expect(expected_calls)
        .mount(&server)
        .await;

    let backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(10))
        .with_factor(2.0)
        .with_max_delay(Duration::from_millis(200));

    let client = Client::builder()
        .base_url(server.uri())
        .api_key(secret_key)
        .build()
        .unwrap()
        .with_backoff(backoff);

    let request = CreateMessagesRequestBuilder::default()
        .model("test-model".to_string())
        .stream(true)
        .messages(vec![
            MessageBuilder::default()
                .role(MessageRole::User)
                .content("Hello world!")
                .build()
                .unwrap(),
        ])
        .build()
        .unwrap();

    let result = client.messages().create(request).await;
    assert!(result.is_ok());
    let result = result.unwrap();

    if let MessageContent::Text(text) = &result.content[0] {
        assert_eq!(text.text, "retried response");
    }
}

#[tokio::test]
async fn test_error_handling_bad_request() {
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock 400 Bad Request response
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "invalid_request_error",
                "message": "Bad request"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .api_key(secret_key)
        .build()
        .unwrap();

    let request = CreateMessagesRequestBuilder::default()
        .model("test-model".to_string())
        .stream(true)
        .messages(vec![
            MessageBuilder::default()
                .role(MessageRole::User)
                .content("Hello world!")
                .build()
                .unwrap(),
        ])
        .build()
        .unwrap();

    let result = client.messages().create(request).await;

    assert!(result.is_err());
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
    let server = TestSetup::setup().await;
    let secret_key = "test_secret";

    // Mock 401 Unauthorized response
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "Unauthorized"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let client = Client::builder()
        .base_url(server.uri())
        .api_key(secret_key)
        .build()
        .unwrap();

    let request = CreateMessagesRequestBuilder::default()
        .model("test-model".to_string())
        .stream(true)
        .messages(vec![
            MessageBuilder::default()
                .role(MessageRole::User)
                .content("Hello world!")
                .build()
                .unwrap(),
        ])
        .build()
        .unwrap();

    let result = client.messages().create(request).await;

    assert!(result.is_err());
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
