//! Tool execution for the query stream pipeline.
//!
//! Manages the full tool lifecycle: coordination of parallel execution,
//! single-tool execution, interactive prompts, and terminal rendering.

pub(crate) mod builtins;
pub(crate) mod coordinator;
pub(crate) mod executor;
pub(crate) mod inquiry;
pub(crate) mod prompter;

pub(crate) use coordinator::{PermissionDecision, ToolCallState, ToolCoordinator};
pub(crate) use executor::TerminalExecutorSource;
pub(crate) use prompter::ToolPrompter;

pub(crate) use crate::{render::ToolRenderer, timer::spawn_line_timer};
