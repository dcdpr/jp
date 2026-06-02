use std::{
    collections::HashSet,
    pin::Pin,
    time::{Duration, SystemTime},
};

use async_stream::stream;
use futures::{Stream, StreamExt as _};
use tokio::time::timeout;

use crate::{
    error::StreamError,
    event::{Event, EventPart, ToolCallPart},
};

pub(super) mod aggregator;
pub(super) mod chain;

/// A stream of events from an LLM provider.
///
/// Errors are represented as `StreamError` to provide provider-agnostic error
/// classification for retry logic.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event, StreamError>> + Send>>;

/// Wrap an event stream so a silent provider connection fails instead of
/// hanging.
///
/// If no item arrives within `idle` of the previous one (or of the initial
/// poll), the returned stream yields a retryable [`StreamErrorKind::Timeout`]
/// error and then ends, letting the retry layer rebuild the stream.
/// The timer resets on every item, so long but active streams are left
/// untouched.
///
/// Idle is measured against the wall clock rather than the monotonic timer, so
/// time the machine spends asleep counts toward the timeout.
/// The monotonic clock pauses during system sleep on macOS and Linux, so a
/// purely monotonic timeout would only resume counting after wake — a stream
/// interrupted by closing the laptop lid would then hang for a further `idle`
/// of awake time before retrying.
/// Measuring against the wall clock and polling on a short cadence detects the
/// resume within ~1s of reopening the lid instead.
///
/// [`StreamErrorKind::Timeout`]: crate::StreamErrorKind::Timeout
#[must_use]
pub fn with_idle_timeout(stream: EventStream, idle: Duration) -> EventStream {
    with_idle_timeout_at(stream, idle, SystemTime::now)
}

/// How often to re-check the wall clock while waiting for the next item.
///
/// This doubles as the post-wake grace window: after the machine resumes from
/// sleep, a still-alive stream has this long to produce data before the idle
/// check fires and reconnects.
/// Kept at a few seconds so the first reconnect isn't attempted before Wi-Fi
/// has had a chance to reassociate, while still detecting a dead connection far
/// faster than the full `idle` window.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(3);

/// [`with_idle_timeout`] with an injectable wall clock, for tests.
fn with_idle_timeout_at(
    stream: EventStream,
    idle: Duration,
    now: impl Fn() -> SystemTime + Send + 'static,
) -> EventStream {
    // Poll cadence: wake often enough to notice a resume from system sleep
    // quickly, but never more often than `idle` itself.
    let tick = idle.min(IDLE_POLL_INTERVAL);

    stream! {
        let mut stream = stream;
        let mut last_activity = now();
        loop {
            match timeout(tick, stream.next()).await {
                Ok(Some(item)) => {
                    last_activity = now();
                    yield item;
                }
                Ok(None) => break,
                Err(_elapsed) => {
                    // The monotonic `tick` elapsed without an item. Decide using
                    // the wall clock, which counts time spent asleep: a lid
                    // reopened after sleep is caught on the first tick after
                    // wake rather than after another full `idle` of awake time.
                    let idle_for = now()
                        .duration_since(last_activity)
                        .unwrap_or(Duration::ZERO);
                    if idle_for >= idle {
                        yield Err(StreamError::timeout(format!(
                            "no activity from provider for {}s",
                            idle_for.as_secs()
                        )));
                        break;
                    }
                }
            }
        }
    }
    .boxed()
}

/// Inject a synthetic [`Event::KeepAlive`] every `interval` while a tool call
/// is mid-stream.
///
/// Some providers emit a large tool-call argument as a burst after a long
/// silent gap.
/// Anthropic streams one complete key/value property at a time, with delays
/// between events while the model works:
/// <https://platform.claude.com/docs/en/build-with-claude/streaming#input-json-delta>
/// Downstream, [`with_idle_timeout`] would read that gap as a dead connection;
/// a keep-alive during the gap keeps it classified as activity.
///
/// Apply this per-provider, only where the provider is known to have such gaps.
/// Providers without them keep their real idle behavior during tool calls, so a
/// genuine stall there is still surfaced as a timeout.
/// Outside an open tool call this is a transparent pass-through.
#[must_use]
pub fn with_tool_call_keepalive(stream: EventStream, interval: Duration) -> EventStream {
    stream! {
        let mut stream = stream;
        let mut open_tool_calls: HashSet<usize> = HashSet::new();
        loop {
            // Outside an open tool call, pass through and let the downstream
            // idle timeout own liveness.
            if open_tool_calls.is_empty() {
                match stream.next().await {
                    Some(item) => {
                        track_tool_calls(&item, &mut open_tool_calls);
                        yield item;
                    }
                    None => break,
                }
                continue;
            }

            match timeout(interval, stream.next()).await {
                Ok(Some(item)) => {
                    track_tool_calls(&item, &mut open_tool_calls);
                    yield item;
                }
                Ok(None) => break,
                // No event within `interval` while a tool call is open: emit a
                // heartbeat so the gap reads as activity, then keep waiting.
                Err(_elapsed) => yield Ok(Event::KeepAlive),
            }
        }
    }
    .boxed()
}

/// Track tool-call open/close boundaries for [`with_tool_call_keepalive`].
///
/// A `ToolCallPart::Start` opens the call at its index; the matching `Flush`
/// closes it.
/// `Finished` clears any still-open calls, since the stream is ending.
fn track_tool_calls(item: &Result<Event, StreamError>, open: &mut HashSet<usize>) {
    let Ok(event) = item else { return };
    match event {
        Event::Part {
            index,
            part: EventPart::ToolCall(ToolCallPart::Start { .. }),
            ..
        } => {
            open.insert(*index);
        }
        Event::Flush { index, .. } => {
            open.remove(index);
        }
        Event::Finished(_) => open.clear(),
        _ => {}
    }
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
