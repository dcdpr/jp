//! Stream processing for the query pipeline.
//!
//! Handles rendering of LLM response chunks and retry logic for transient
//! stream errors.

pub(crate) mod renderer;
pub(crate) mod retry;
pub(crate) mod structured_renderer;

pub(crate) use renderer::ChatResponseRenderer;
pub(crate) use retry::{StreamRetryState, handle_stream_error};
pub(crate) use structured_renderer::StructuredRenderer;
