use std::pin::Pin;

use futures::Stream;

use crate::{Error, event::Event};

pub(super) mod aggregator;
pub(super) mod chain;

pub type EventStream = Pin<Box<dyn Stream<Item = Result<Event, Error>> + Send>>;
