pub mod error;
pub mod event;
pub mod model;
pub mod provider;
pub mod query;
pub mod retry;
mod stream;
pub mod title;
pub mod tool;

#[cfg(test)]
pub(crate) mod test;

pub use error::{Error, StreamError, StreamErrorKind, ToolError};
pub use provider::Provider;
pub use retry::exponential_backoff;
pub use stream::{EventStream, aggregator::tool_call_request::AggregationError, chain::EventChain};
pub use tool::{CommandResult, ExecutionOutcome, run_tool_command};
