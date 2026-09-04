//! Interrupt and signal handling for the query stream pipeline.
//!
//! Provides context-aware interrupt menus (streaming vs tool execution) and
//! routes OS signals to the appropriate handlers.

pub(crate) mod channel;
pub(crate) mod handler;
pub(crate) mod signals;

pub(crate) use channel::{TurnInterruptSender, TurnInterrupts};
pub(crate) use handler::{InterruptAction, reply_edit_mode};
pub(crate) use signals::{
    LoopAction, StreamingInterruptResult, apply_streaming_interrupt, handle_llm_event,
    handle_streaming_interrupt,
};
