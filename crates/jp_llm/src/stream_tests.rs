use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures::{StreamExt as _, future, stream};
use serde_json::Map;

use super::{
    output_limit_bytes, with_idle_timeout, with_idle_timeout_at, with_output_limit,
    with_tool_call_keepalive,
};
use crate::{
    StreamError, StreamErrorKind,
    event::{Event, EventPart, FinishReason},
};

#[tokio::test(start_paused = true)]
async fn idle_timeout_fires_when_wall_clock_exceeds_idle() {
    // Simulate a system suspend: the wall clock jumps forward by 120s between
    // the initial poll and the first tick after "wake", while the monotonic
    // timer that `start_paused` drives only advances by one tick. This is the
    // lid-close scenario — the timeout must fire on the first post-wake tick,
    // not after another full idle window of awake time.
    let base = SystemTime::UNIX_EPOCH;
    let calls = Arc::new(AtomicUsize::new(0));
    let now = move || {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            base
        } else {
            base + Duration::from_mins(2)
        }
    };

    let inner = stream::pending::<Result<Event, StreamError>>().boxed();
    let mut wrapped = with_idle_timeout_at(inner, Duration::from_secs(5), now);

    let err = wrapped
        .next()
        .await
        .expect("an item before the stream ends")
        .expect_err("a timeout error");
    assert_eq!(err.kind, StreamErrorKind::Timeout);

    assert!(
        wrapped.next().await.is_none(),
        "stream ends after the idle timeout fires"
    );
}

#[tokio::test(start_paused = true)]
async fn active_stream_passes_through_without_timeout() {
    let inner = stream::iter(vec![Ok(Event::flush(0)), Ok(Event::flush(1))]).boxed();
    let mut wrapped = with_idle_timeout(inner, Duration::from_secs(5));

    assert!(wrapped.next().await.expect("first item").is_ok());
    assert!(wrapped.next().await.expect("second item").is_ok());
    assert!(wrapped.next().await.is_none(), "inner stream exhausted");
}

#[tokio::test]
async fn output_limit_fires_once_the_ceiling_is_crossed() {
    // Four 10-byte parts against a 25-byte ceiling. The third crosses it, so
    // exactly three parts are forwarded before the guard errors: the crossing
    // part is still delivered, because content already generated is never
    // withheld. The fourth part is never polled.
    let inner = stream::iter(vec![
        Ok(Event::reasoning(0, "0123456789")),
        Ok(Event::message(1, "0123456789")),
        Ok(Event::structured(2, "0123456789")),
        Ok(Event::tool_call_args(3, "0123456789")),
    ])
    .boxed();
    let mut wrapped = with_output_limit(inner, 25);

    for index in 0..3 {
        assert!(
            wrapped.next().await.expect("a forwarded part").is_ok(),
            "part {index} is forwarded"
        );
    }

    let err = wrapped
        .next()
        .await
        .expect("an item before the stream ends")
        .expect_err("an output limit error");
    assert_eq!(err.kind, StreamErrorKind::OutputLimit);
    assert!(
        !err.is_retryable(),
        "regenerating a runaway response must not be retried"
    );

    assert!(
        wrapped.next().await.is_none(),
        "stream ends after the output limit fires"
    );
}

#[tokio::test]
async fn output_under_the_ceiling_passes_through_untouched() {
    let inner = stream::iter(vec![
        Ok(Event::message(0, "0123456789")),
        Ok(Event::flush(0)),
        Ok(Event::Finished(FinishReason::Completed)),
    ])
    .boxed();
    let mut wrapped = with_output_limit(inner, 25);

    assert!(matches!(wrapped.next().await, Some(Ok(Event::Part { .. }))));
    assert!(matches!(
        wrapped.next().await,
        Some(Ok(Event::Flush { .. }))
    ));
    assert!(matches!(
        wrapped.next().await,
        Some(Ok(Event::Finished(FinishReason::Completed)))
    ));
    assert!(wrapped.next().await.is_none(), "inner stream exhausted");
}

#[tokio::test]
async fn non_content_events_do_not_count_toward_the_ceiling() {
    // Keep-alives, patches, and metadata-free flushes carry nothing generated,
    // so a stream of nothing but those must survive a 1-byte ceiling.
    let inner = stream::iter(vec![
        Ok(Event::flush(0)),
        Ok(Event::KeepAlive),
        Ok(Event::Patch(vec![])),
        Ok(Event::flush(1)),
    ])
    .boxed();

    let items: Vec<_> = with_output_limit(inner, 1).collect().await;

    assert_eq!(items.len(), 4, "no extra error item is appended");
    assert!(items.iter().all(Result::is_ok), "got: {items:?}");
}

#[tokio::test]
async fn retained_part_metadata_counts_toward_the_ceiling() {
    // Anthropic delivers redacted thinking as an empty reasoning part with the
    // payload in metadata. Counting only the content field would let an
    // arbitrarily large retained payload through at zero counted bytes.
    let payload = "0123456789".repeat(4);
    let inner = stream::iter(vec![
        Ok(Event::Part {
            index: 0,
            part: EventPart::Reasoning(String::new()),
            metadata: Map::from_iter([(
                "anthropic_redacted_thinking".to_owned(),
                payload.clone().into(),
            )]),
        }),
        Ok(Event::Part {
            index: 0,
            part: EventPart::Reasoning(String::new()),
            metadata: Map::from_iter([("anthropic_redacted_thinking".to_owned(), payload.into())]),
        }),
    ])
    .boxed();

    // 40 metadata bytes per part against a 50-byte ceiling: the second crosses.
    let mut wrapped = with_output_limit(inner, 50);

    assert!(
        wrapped.next().await.expect("the first part").is_ok(),
        "the first part is under the ceiling"
    );
    assert!(wrapped.next().await.expect("the second part").is_ok());

    let err = wrapped
        .next()
        .await
        .expect("an item before the stream ends")
        .expect_err("an output limit error");
    assert_eq!(err.kind, StreamErrorKind::OutputLimit);
}

#[tokio::test]
async fn retained_flush_metadata_counts_toward_the_ceiling() {
    // OpenAI attaches encrypted reasoning content to the flush rather than to a
    // part, so a flush is not automatically free.
    let inner = stream::iter(vec![Ok(Event::flush_with_metadata_field(
        0,
        "openai_encrypted_content",
        "0123456789".repeat(4),
    ))])
    .boxed();

    let mut wrapped = with_output_limit(inner, 25);

    assert!(
        wrapped.next().await.expect("the flush").is_ok(),
        "the flush is forwarded before the guard errors"
    );

    let err = wrapped
        .next()
        .await
        .expect("an item before the stream ends")
        .expect_err("an output limit error");
    assert_eq!(err.kind, StreamErrorKind::OutputLimit);
}

#[tokio::test]
async fn repeated_tool_call_openings_reach_the_ceiling() {
    // A tool call's `id` and `name` are generated response bytes. A model that
    // emits nothing but empty tool calls must still hit the ceiling, rather
    // than streaming forever at zero counted bytes.
    let inner = stream::iter(
        std::iter::repeat_with(|| Ok(Event::tool_call_start(0, "call_01", "some_tool")))
            .take(20)
            .collect::<Vec<_>>(),
    )
    .boxed();

    // 16 bytes per opening (7 + 9) against a 40-byte ceiling: the third crosses
    // it at 48 bytes.
    let mut wrapped = with_output_limit(inner, 40);

    for index in 0..3 {
        assert!(
            wrapped.next().await.expect("a forwarded part").is_ok(),
            "tool call opening {index} is forwarded"
        );
    }

    let err = wrapped
        .next()
        .await
        .expect("an item before the stream ends")
        .expect_err("an output limit error");
    assert_eq!(err.kind, StreamErrorKind::OutputLimit);

    assert!(
        wrapped.next().await.is_none(),
        "stream ends after the output limit fires"
    );
}

#[test]
fn output_ceiling_of_zero_is_disabled() {
    // `0` means "no ceiling" at the config layer. Both call sites route through
    // this translation so they cannot drift on what `0` means.
    assert!(output_limit_bytes(0).is_none(), "zero disables the ceiling");
    assert_eq!(
        output_limit_bytes(512),
        Some(512),
        "a non-zero ceiling is used as-is"
    );
}

#[tokio::test(start_paused = true)]
async fn tool_call_keepalive_emitted_during_open_tool_call() {
    // A tool-call Start opens the call; the model then goes silent for longer
    // than the keepalive interval before the matching Flush. A KeepAlive must
    // be injected during the gap so a downstream idle timeout sees activity.
    let inner = stream::once(future::ready(Ok(Event::tool_call_start(0, "id", "name"))))
        .chain(stream::once(async {
            tokio::time::sleep(Duration::from_secs(8)).await;
            Ok(Event::flush(0))
        }))
        .boxed();
    let mut wrapped = with_tool_call_keepalive(inner, Duration::from_secs(5));

    assert!(matches!(wrapped.next().await, Some(Ok(Event::Part { .. }))));
    assert!(
        matches!(wrapped.next().await, Some(Ok(Event::KeepAlive))),
        "a keepalive is injected during the gap"
    );
    assert!(matches!(
        wrapped.next().await,
        Some(Ok(Event::Flush { .. }))
    ));
    assert!(wrapped.next().await.is_none(), "inner stream exhausted");
}

#[tokio::test(start_paused = true)]
async fn no_keepalive_outside_tool_call() {
    // No tool call is open, so a long gap passes through untouched; the
    // downstream idle timeout owns liveness in this window.
    let inner = stream::once(future::ready(Ok(Event::message(0, "a"))))
        .chain(stream::once(async {
            tokio::time::sleep(Duration::from_secs(8)).await;
            Ok(Event::message(1, "b"))
        }))
        .boxed();
    let mut wrapped = with_tool_call_keepalive(inner, Duration::from_secs(5));

    assert!(matches!(wrapped.next().await, Some(Ok(Event::Part { .. }))));
    assert!(
        matches!(wrapped.next().await, Some(Ok(Event::Part { .. }))),
        "the gap is passed through without a keepalive"
    );
    assert!(wrapped.next().await.is_none(), "inner stream exhausted");
}
