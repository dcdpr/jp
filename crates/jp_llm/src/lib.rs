pub mod error;
pub mod event;
pub mod event_builder;
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
pub use stream::{EventStream, chain::EventChain};
pub use tool::{CommandResult, ExecutionOutcome, ToolTrace, run_tool_command};
