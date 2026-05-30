use std::{pin::Pin, time::Duration};

use async_stream::stream;
use futures::{Stream, StreamExt as _};
use tokio::time::timeout;

use crate::{error::StreamError, event::Event};

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
/// [`StreamErrorKind::Timeout`]: crate::StreamErrorKind::Timeout
#[must_use]
pub fn with_idle_timeout(stream: EventStream, idle: Duration) -> EventStream {
    stream! {
        let mut stream = stream;
        loop {
            match timeout(idle, stream.next()).await {
                Ok(Some(item)) => yield item,
                Ok(None) => break,
                Err(_elapsed) => {
                    yield Err(StreamError::timeout(format!(
                        "no activity from provider for {}s",
                        idle.as_secs()
                    )));
                    break;
                }
            }
        }
    }
    .boxed()
}

#[cfg(test)]
#[path = "stream_tests.rs"]
mod tests;
