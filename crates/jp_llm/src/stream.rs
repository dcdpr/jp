use std::pin::Pin;

use futures::Stream;

use crate::{error::StreamError, event::Event};

pub(super) mod aggregator;
pub(super) mod chain;

/// A stream of events from an LLM provider.
///
/// Errors are represented as `StreamError` to provide provider-agnostic error
/// classification for retry logic.
pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event, StreamError>> + Send>>;
