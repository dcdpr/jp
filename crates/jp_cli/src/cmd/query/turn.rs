//! Turn management for the query stream pipeline.
//!
//! A "turn" is the complete interaction from user query to final response,
//! consisting of one or more request-response cycles with the LLM.

pub(crate) mod coordinator;
pub(crate) mod state;

pub(crate) use coordinator::{Action, TurnCoordinator, TurnPhase};
pub(crate) use state::TurnState;
