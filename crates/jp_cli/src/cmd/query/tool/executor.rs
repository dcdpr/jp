//! Single tool execution for the query stream pipeline.
//!
//! The `ToolExecutor` handles execution of a single tool call, including:
//! - Permission prompts (run mode configuration)
//! - Input prompts (tool-specific questions)
//! - Result formatting
//!
//! # Lifecycle State Machine
//!
//! ```text
//!                     ┌─────────────────────────────────────────────────────┐
//!                     │                  ToolExecutor                       │
//!                     │                                                     │
//!   ┌─────────┐       │  ┌─────────┐    ┌──────────────────┐    ┌─────────┐ │
//!   │ new()   │──────▶│  │ Pending │───▶│AwaitingPermission│───▶│ Running │ │
//!   └─────────┘       │  └─────────┘    └──────────────────┘    └────┬────┘ │
//!                     │                         │                    │      │
//!                     │                         │ (skip)             │      │
//!                     │                         ▼                    ▼      │
//!                     │                   ┌───────────┐      ┌─────────────┐│
//!                     │                   │ Completed │◀─────│AwaitingInput││
//!                     │                   └───────────┘      └─────────────┘│
//!                     │                         ▲                    │      │
//!                     │                         │                    │      │
//!                     │                   ┌───────────────────┐      │      │
//!                     │                   │AwaitingResultEdit │◀─────┘      │
//!                     │                   └───────────────────┘             │
//!                     └─────────────────────────────────────────────────────┘
//! ```
//!
//! # Thread Safety
//!
//! The executor works with `SharedTurnState` (`Arc<RwLock<TurnState>>`) to
//! support parallel execution. Lock durations are minimized to avoid
//! blocking other executors.
//!
//! # Testing
//!
//! The [`Executor`] trait allows for mock implementations in tests.
//! See [`MockExecutor`] for testing parallel execution behavior.
//!
//! [`MockExecutor`]: jp_llm::tool::executor::MockExecutor

use std::sync::Arc;

use async_trait::async_trait;
use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::conversation::tool::{RunMode, ToolConfigWithDefaults, ToolsConfig};
use jp_conversation::event::{ToolCallRequest, ToolCallResponse};
use jp_llm::{
    ExecutionOutcome, ToolError,
    tool::{
        ToolDefinition,
        builtin::BuiltinExecutors,
        executor::{Executor, ExecutorResult, ExecutorSource, PermissionInfo},
    },
};
use jp_mcp::Client;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Terminal executor source that creates real [`ToolExecutor`] instances.
///
/// Holds pre-resolved tool definitions so executors don't need to re-resolve
/// (avoiding redundant MCP server fetches).
pub struct TerminalExecutorSource {
    builtin_executors: BuiltinExecutors,
    definitions: IndexMap<String, ToolDefinition>,
}

impl TerminalExecutorSource {
    #[must_use]
    pub fn new(builtin_executors: BuiltinExecutors, definitions: &[ToolDefinition]) -> Self {
        let definitions = definitions
            .iter()
            .map(|d| (d.name.clone(), d.clone()))
            .collect();
        Self {
            builtin_executors,
            definitions,
        }
    }
}

#[async_trait]
impl ExecutorSource for TerminalExecutorSource {
    async fn create(
        &self,
        request: ToolCallRequest,
        tools_config: &ToolsConfig,
        _mcp_client: &Client,
    ) -> Result<Box<dyn Executor>, ToolError> {
        Ok(Box::new(ToolExecutor::new(
            request,
            tools_config,
            &self.definitions,
            Arc::new(self.builtin_executors.clone()),
        )?))
    }
}

/// Executes a single tool call.
///
/// The executor handles the execution lifecycle including permission prompts,
/// input questions, and result formatting.
///
/// # Note
///
/// Interactive prompts currently happen inside `ToolDefinition::call()`.
/// In the future, prompts will be driven by the `ToolCoordinator`, and
/// the executor will only handle pure execution.
pub struct ToolExecutor {
    request: ToolCallRequest,
    config: ToolConfigWithDefaults,
    definition: ToolDefinition,
    builtin_executors: Arc<BuiltinExecutors>,
}

impl ToolExecutor {
    /// Creates a new executor for the given tool call request.
    ///
    /// Uses a pre-resolved definition from the `definitions` map instead of
    /// re-resolving from config + MCP.
    pub fn new(
        request: ToolCallRequest,
        tools_config: &ToolsConfig,
        definitions: &IndexMap<String, ToolDefinition>,
        builtin_executors: Arc<BuiltinExecutors>,
    ) -> Result<Self, ToolError> {
        let tool_config = tools_config
            .get(&request.name)
            .ok_or_else(|| ToolError::NotFound {
                name: request.name.clone(),
            })?;

        let definition = definitions
            .get(&request.name)
            .ok_or_else(|| ToolError::NotFound {
                name: request.name.clone(),
            })?
            .clone();

        Ok(Self {
            request,
            config: tool_config.clone(),
            definition,
            builtin_executors,
        })
    }
}

#[async_trait]
impl Executor for ToolExecutor {
    fn tool_id(&self) -> &str {
        &self.request.id
    }

    fn tool_name(&self) -> &str {
        &self.request.name
    }

    fn arguments(&self) -> &serde_json::Map<String, Value> {
        &self.request.arguments
    }

    fn permission_info(&self) -> Option<PermissionInfo> {
        let run_mode = self.config.run();

        // No prompt needed for these modes
        if matches!(run_mode, RunMode::Unattended | RunMode::Skip) {
            return None;
        }

        Some(PermissionInfo {
            tool_id: self.request.id.clone(),
            tool_name: self.request.name.clone(),
            tool_source: self.config.source().clone(),
            run_mode,
            arguments: self.request.arguments.clone().into(),
        })
    }

    fn set_arguments(&mut self, args: Value) {
        if let Value::Object(map) = args {
            self.request.arguments = map;
        }
        // If not an object, ignore (preserve original arguments)
    }

    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        mcp_client: &Client,
        root: &Utf8Path,
        cancellation_token: CancellationToken,
    ) -> ExecutorResult {
        let result = self
            .definition
            .execute(
                self.request.id.clone(),
                Value::Object(self.request.arguments.clone()),
                answers,
                &self.config,
                mcp_client,
                root,
                cancellation_token,
                &self.builtin_executors,
            )
            .await;

        match result {
            Ok(ExecutionOutcome::Completed { id, result }) => {
                ExecutorResult::Completed(ToolCallResponse { id, result })
            }
            Ok(ExecutionOutcome::Cancelled { id }) => ExecutorResult::Completed(ToolCallResponse {
                id,
                result: Ok("Tool execution cancelled.".to_string()),
            }),
            Ok(ExecutionOutcome::NeedsInput { id: _, question }) => ExecutorResult::NeedsInput {
                tool_id: self.request.id.clone(),
                tool_name: self.request.name.clone(),
                question,
                accumulated_answers: answers.clone(),
            },
            Err(e) => ExecutorResult::Completed(ToolCallResponse {
                id: self.request.id.clone(),
                result: Err(e.to_string()),
            }),
        }
    }
}
