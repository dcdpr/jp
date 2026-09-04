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

    assert_eq!(d_high, Duration::from_secs(TEST_MAX_BACKOFF_SECS));
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
    assert_eq!(d_capped, Duration::from_secs(5));
}

/// The window a jittered delay is allowed to land in: at least the base, and
/// less than a quarter above it.
fn assert_within_jitter_window(actual: Duration, base: Duration) {
    let ceiling = base + base / 4;
    assert!(
        actual >= base && actual < ceiling,
        "{actual:?} outside [{base:?}, {ceiling:?})"
    );
}

/// A provider delay below the cap is passed through untouched, and jitter is
/// only ever added to it, never subtracted.
#[test]
fn retry_delay_only_ever_adds_to_a_provider_delay() {
    for _ in 0..200 {
        let delay = retry_delay(
            Some(Duration::from_secs(30)),
            1,
            TEST_BASE_BACKOFF_MS,
            TEST_MAX_BACKOFF_SECS,
        );

        assert_within_jitter_window(delay, Duration::from_secs(30));
    }
}

/// Cerebras answers a rate limit with `retry-after: 60`, so several sessions
/// sharing one API key are handed the identical delay at the same moment.
/// Here it is above the cap, which pins every one of them to the same ceiling:
/// the case where jitter folded inside the bound would vanish and they would
/// all resume in the same instant, triggering the limit again.
///
/// Asserting only the window would pass if jitter never fired at all, so this
/// also pins that repeated calls actually differ.
#[test]
fn retry_delay_spreads_concurrent_sessions_at_the_ceiling() {
    let delays: Vec<_> = (0..200)
        .map(|_| retry_delay(Some(Duration::from_mins(1)), 1, TEST_BASE_BACKOFF_MS, 30))
        .collect();

    let distinct: std::collections::BTreeSet<_> = delays.iter().collect();
    assert!(
        distinct.len() > 100,
        "200 delays produced only {} distinct values; they resume together",
        distinct.len()
    );
}

/// `max_backoff_secs` wins over a longer `retry_after`, so a provider asking
/// for ten minutes against a one-minute cap is waited out in about a minute.
#[test]
fn retry_delay_bounds_a_long_provider_delay() {
    let delay = retry_delay(Some(Duration::from_mins(10)), 1, TEST_BASE_BACKOFF_MS, 60);

    assert_within_jitter_window(delay, Duration::from_mins(1));
}

/// Without provider guidance the delay grows exponentially, and each attempt
/// stays inside its own window.
#[test]
fn retry_delay_falls_back_to_exponential_backoff() {
    for (attempt, base_ms) in [(1, 1000), (2, 2000), (3, 4000)] {
        let delay = retry_delay(None, attempt, TEST_BASE_BACKOFF_MS, TEST_MAX_BACKOFF_SECS);

        assert_within_jitter_window(delay, Duration::from_millis(base_ms));
    }
}

/// A delay too short to carve a jitter window out of is returned unchanged.
/// The naive implementation asks for a random value in `0..0`, which panics.
#[test]
fn retry_delay_leaves_a_sub_millisecond_window_alone() {
    let delay = retry_delay(Some(Duration::from_millis(3)), 1, 1, 60);

    assert_eq!(delay, Duration::from_millis(3));
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
