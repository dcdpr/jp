//! Tool execution for the query stream pipeline.
//!
//! Manages the full tool lifecycle: coordination of parallel execution,
//! single-tool execution, interactive prompts, and terminal rendering.

pub(crate) mod builtins;
pub(crate) mod coordinator;
pub(crate) mod executor;
pub(crate) mod inquiry;
pub(crate) mod mcp_executor;
pub(crate) mod pending;
pub(crate) mod prompter;

pub(crate) use coordinator::{Host, ToolCallState, ToolCoordinator, ToolEvent};
pub(crate) use mcp_executor::TerminalExecutorSource;
pub(crate) use pending::{PendingTools, build_execution_plan, unresponded_requests};
pub(crate) use prompter::ToolPrompter;

pub(crate) use crate::render::ToolRenderer;
