mod error;
pub mod event;
pub mod model;
pub mod provider;
pub mod query;
mod stream;
pub mod structured;
pub mod tool;

#[cfg(test)]
pub(crate) mod test;

pub use error::{Error, ToolError};
pub use provider::Provider;
pub use stream::{aggregator::tool_call_request::AggregationError, chain::EventChain};
