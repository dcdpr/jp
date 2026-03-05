//! Interrupt and signal handling for the query stream pipeline.
//!
//! Provides context-aware interrupt menus (streaming vs tool execution) and
//! routes OS signals to the appropriate handlers.

pub(crate) mod handler;
pub(crate) mod signals;

pub(crate) use handler::InterruptAction;
pub(crate) use signals::{LoopAction, handle_llm_event, handle_streaming_signal};
