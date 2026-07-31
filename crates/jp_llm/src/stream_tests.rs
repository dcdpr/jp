use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use futures::{StreamExt as _, future, stream};

use super::{
    output_limit_bytes, with_idle_timeout, with_idle_timeout_at, with_output_limit,
    with_tool_call_keepalive,
};
use crate::{
    StreamError, StreamErrorKind,
    event::{Event, FinishReason},
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
    // Flushes, keep-alives, and tool-call starts carry no generated content, so
    // a stream of nothing but those must survive a 1-byte ceiling.
    let inner = stream::iter(vec![
        Ok(Event::flush(0)),
        Ok(Event::KeepAlive),
        Ok(Event::tool_call_start(1, "id", "name")),
        Ok(Event::flush(1)),
    ])
    .boxed();

    let items: Vec<_> = with_output_limit(inner, 1).collect().await;

    assert_eq!(items.len(), 4, "no extra error item is appended");
    assert!(items.iter().all(Result::is_ok), "got: {items:?}");
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
