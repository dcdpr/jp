use jp_config::{
    assistant::tool_choice::ToolChoice,
    model::id::{ModelIdConfig, ProviderId},
};
use jp_conversation::{ConversationStream, thread::Thread};

use super::*;
use crate::{
    error::{Error, StreamError, StreamErrorKind},
    event::Event,
    model::ModelDetails,
    provider::mock::MockProvider,
};

fn empty_query() -> ChatQuery {
    ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test(),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    }
}

fn model() -> ModelDetails {
    ModelDetails::empty(ModelIdConfig {
        provider: ProviderId::Test,
        name: "mock-model".parse().expect("valid model name"),
    })
}

/// Default base backoff for tests.
const TEST_BASE_BACKOFF_MS: u64 = 1000;

/// Default max backoff for tests.
const TEST_MAX_BACKOFF_SECS: u64 = 60;

#[test]
fn backoff_increases() {
    let d1 = exponential_backoff(1, TEST_BASE_BACKOFF_MS, TEST_MAX_BACKOFF_SECS);
    let d2 = exponential_backoff(2, TEST_BASE_BACKOFF_MS, TEST_MAX_BACKOFF_SECS);
    let d3 = exponential_backoff(3, TEST_BASE_BACKOFF_MS, TEST_MAX_BACKOFF_SECS);

    // Base delays should roughly double
    // attempt 1: ~1000ms, attempt 2: ~2000ms, attempt 3: ~4000ms
    assert!(d1 < d2);
    assert!(d2 < d3);
}

#[test]
fn backoff_capped() {
    let d_high = exponential_backoff(100, TEST_BASE_BACKOFF_MS, TEST_MAX_BACKOFF_SECS);

    // Should be capped at max_backoff_secs
    assert!(d_high <= Duration::from_secs(TEST_MAX_BACKOFF_SECS + 1));
}

#[test]
fn backoff_respects_config() {
    // Custom base and max
    let d1 = exponential_backoff(1, 500, 10);
    let d2 = exponential_backoff(1, 2000, 10);

    // Higher base should give higher delay
    assert!(d1 < d2);

    // Should respect max cap
    let d_capped = exponential_backoff(100, 1000, 5);
    assert!(d_capped <= Duration::from_secs(5));
}

#[test]
fn stream_error_is_retryable() {
    // Retryable error kinds
    assert!(StreamError::timeout("test").is_retryable());
    assert!(StreamError::connect("test").is_retryable());
    assert!(StreamError::rate_limit(None).is_retryable());
    assert!(StreamError::transient("test").is_retryable());

    // Non-retryable
    assert!(!StreamError::other("test").is_retryable());
}

#[tokio::test]
async fn collect_with_retry_applies_the_output_ceiling_without_retrying() {
    // One scripted request is deliberate. If OutputLimit is misclassified as
    // retryable, the mock panics when collect_with_retry asks for a second one.
    let provider = MockProvider::with_batches(vec![vec![
        Event::message(0, "0123456789"),
        Event::message(0, "0123456789"),
        Event::message(0, "0123456789"),
    ]]);
    let config = RetryConfig {
        max_retries: 5,
        base_backoff_ms: 1,
        max_backoff_secs: 1,
        max_response_bytes: Some(25),
    };

    let error = collect_with_retry(&provider, &model(), empty_query(), &config)
        .await
        .expect_err("response must stop at the configured ceiling");

    assert!(
        matches!(
            error,
            Error::Stream(StreamError {
                kind: StreamErrorKind::OutputLimit,
                ..
            })
        ),
        "got: {error:?}"
    );
}

#[test]
fn stream_error_with_retry_after() {
    let err = StreamError::rate_limit(Some(Duration::from_secs(30)));
    assert_eq!(err.retry_after, Some(Duration::from_secs(30)));
    assert!(err.is_retryable());
}
