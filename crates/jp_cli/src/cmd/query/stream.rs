//! Stream processing for the query pipeline.
//!
//! Handles rendering of LLM response chunks and retry logic for transient
//! stream errors.

pub(crate) mod retry;

pub(crate) use retry::{
    RebuildRefusal, ResponseBoundary, StreamErrorOutcome, StreamRetryState, agent_restart_is_safe,
    commit_partial_response, handle_stream_error,
};

pub(crate) use crate::render::TurnView;
