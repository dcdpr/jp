use std::sync::Mutex;

use async_trait::async_trait;
use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::conversation::tool::{RunMode, ToolSource, ToolsConfig};
use jp_conversation::event::{ToolCallRequest, ToolCallResponse};
use jp_mcp::Client;
use jp_tool::Question;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::ToolError;

/// Trait for tool execution, enabling mock implementations for testing.
///
/// This trait abstracts the execution of a single tool call, allowing the
/// `ToolCoordinator` to work with both real and mock executors.
///
/// # Design
///
/// The executor is intentionally simple - it just executes tools with given
/// answers. All decision-making about question targets, static answers, and how
/// to handle `NeedsInput` is done by the coordinator, which has access to the
/// tool configuration.
#[async_trait]
pub trait Executor: Send + Sync {
    /// Returns the tool call ID.
    fn tool_id(&self) -> &str;

    /// Returns the tool name.
    fn tool_name(&self) -> &str;

    /// Returns the tool call arguments.
    ///
    /// This is separate from [`permission_info()`](Self::permission_info)
    /// because arguments are always available, while permission info is only
    /// present for tools that require a permission prompt.
    fn arguments(&self) -> &Map<String, Value>;

    /// Returns information needed for permission prompting.
    ///
    /// Returns `None` if the tool doesn't need a permission prompt (e.g.,
    /// `RunMode::Unattended` or `RunMode::Skip`).
    fn permission_info(&self) -> Option<PermissionInfo>;

    /// Updates the arguments to use for execution.
    ///
    /// This is called after permission prompting if the user edited the
    /// arguments (via `RunMode::Edit`). The new arguments replace the original
    /// arguments from the tool call request.
    fn set_arguments(&mut self, args: Value);

    /// Executes the tool once with the given answers.
    ///
    /// This method performs a single execution pass. If the tool needs
    /// additional input, it returns `ExecutorResult::NeedsInput` and the
    /// coordinator handles prompting and retrying.
    ///
    /// The executor doesn't know how questions should be answered - it just
    /// reports that input is needed. The coordinator looks up the tool
    /// configuration to determine whether to prompt the user or ask the LLM.
    ///
    /// # Arguments
    ///
    /// * `answers` - Accumulated answers from previous `NeedsInput` responses
    /// * `mcp_client` - MCP client for remote tool execution
    /// * `root` - Project root directory
    /// * `cancellation_token` - Token to cancel execution
    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        mcp_client: &Client,
        root: &Utf8Path,
        cancellation_token: CancellationToken,
    ) -> ExecutorResult;
}

/// Abstraction over how executors are created for tool calls.
///
/// This trait enables dependency injection of executor creation, allowing tests
/// to use mock executors without executing real shell commands.
#[async_trait]
pub trait ExecutorSource: Send + Sync {
    /// Creates an executor for the given tool call request.
    ///
    /// # Arguments
    ///
    /// * `request` - The tool call request from the LLM
    /// * `tools_config` - Configuration for all tools
    /// * `mcp_client` - MCP client for remote tool execution
    ///
    /// # Errors
    ///
    /// Returns an error if the tool is not found or cannot be initialized.
    async fn create(
        &self,
        request: ToolCallRequest,
        tools_config: &ToolsConfig,
        mcp_client: &Client,
    ) -> Result<Box<dyn Executor>, ToolError>;
}

/// Result of a tool execution attempt.
///
/// Tools may need multiple rounds of execution if they require additional
/// input. This enum allows the executor to return control to the coordinator,
/// which decides how to handle the `NeedsInput` case by looking up the question
/// configuration.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // NeedsInput variant is larger but rarely used
pub enum ExecutorResult {
    /// Tool completed (success or error).
    Completed(ToolCallResponse),

    /// Tool needs additional input before it can continue.
    ///
    /// The executor doesn't know who should answer - it just reports that input
    /// is needed. The coordinator looks up the question configuration to
    /// determine the target:
    ///
    /// - `User`: Prompt the user interactively, then restart the tool
    /// - `Assistant`: Format a response asking the LLM to re-run with answers
    NeedsInput {
        /// Tool call ID.
        tool_id: String,

        /// Tool name (for persisting answers).
        tool_name: String,

        /// The question that needs to be answered.
        question: Question,

        /// Accumulated answers so far (for retry).
        accumulated_answers: IndexMap<String, Value>,
    },
}

/// A mock executor for testing that returns pre-configured results.
///
/// This executor doesn't execute any real commands - it simply returns whatever
/// result is configured, making it ideal for testing tool coordination flows
/// without side effects.
///
/// # Example
///
/// ```ignore
/// let executor = MockExecutor::completed("call_1", "my_tool", "success output");
/// let result = executor.execute(&answers, &client, &root, token).await;
/// assert!(result.is_completed());
/// ```
pub struct MockExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,
    permission_info: Option<PermissionInfo>,
    result: Mutex<Option<ExecutorResult>>,
}

impl MockExecutor {
    /// Creates a mock executor that returns a successful completion.
    #[must_use]
    pub fn completed(tool_id: &str, tool_name: &str, output: &str) -> Self {
        Self {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: Map::new(),
            permission_info: None,
            result: Mutex::new(Some(ExecutorResult::Completed(ToolCallResponse {
                id: tool_id.to_string(),
                result: Ok(output.to_string()),
            }))),
        }
    }

    /// Creates a mock executor that returns an error.
    #[must_use]
    pub fn error(tool_id: &str, tool_name: &str, error: &str) -> Self {
        Self {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: Map::new(),
            permission_info: None,
            result: Mutex::new(Some(ExecutorResult::Completed(ToolCallResponse {
                id: tool_id.to_string(),
                result: Err(error.to_string()),
            }))),
        }
    }

    /// Sets the arguments for this executor.
    #[must_use]
    pub fn with_arguments(mut self, args: Map<String, Value>) -> Self {
        self.arguments = args;
        self
    }

    /// Sets the permission info for this executor.
    ///
    /// If set, the executor will require permission prompting based on the
    /// configured `RunMode`.
    #[must_use]
    pub fn with_permission_info(mut self, info: PermissionInfo) -> Self {
        self.permission_info = Some(info);
        self
    }

    /// Sets a custom result for this executor.
    #[must_use]
    pub fn with_result(mut self, result: ExecutorResult) -> Self {
        self.result = Mutex::new(Some(result));
        self
    }
}

#[async_trait]
impl Executor for MockExecutor {
    fn tool_id(&self) -> &str {
        &self.tool_id
    }

    fn tool_name(&self) -> &str {
        &self.tool_name
    }

    fn arguments(&self) -> &Map<String, Value> {
        &self.arguments
    }

    fn permission_info(&self) -> Option<PermissionInfo> {
        self.permission_info.clone()
    }

    fn set_arguments(&mut self, _args: Value) {
        // No-op for mock executor - arguments don't affect the pre-configured
        // result
    }

    async fn execute(
        &self,
        _answers: &IndexMap<String, Value>,
        _mcp_client: &Client,
        _root: &Utf8Path,
        _cancellation_token: CancellationToken,
    ) -> ExecutorResult {
        self.result.lock().unwrap().take().unwrap_or_else(|| {
            ExecutorResult::Completed(ToolCallResponse {
                id: self.tool_id.clone(),
                result: Err("MockExecutor: result already consumed".to_string()),
            })
        })
    }
}

/// An executor source for testing that returns pre-registered mock executors.
///
/// This allows tests to inject mock executors for specific tool names without
/// executing any real shell commands.
///
/// # Example
///
/// ```ignore
/// let source = TestExecutorSource::new()
///     .with_executor("my_tool", |req| {
///         Box::new(MockExecutor::completed(&req.id, &req.name, "mock output"))
///     });
///
/// let coordinator = ToolCoordinator::new(tools_config, Arc::new(source));
/// ```
pub struct TestExecutorSource {
    #[allow(clippy::type_complexity)]
    factories: std::collections::HashMap<
        String,
        Box<dyn Fn(ToolCallRequest) -> Box<dyn Executor> + Send + Sync>,
    >,
}

impl TestExecutorSource {
    /// Creates a new empty test executor source.
    #[must_use]
    pub fn new() -> Self {
        Self {
            factories: std::collections::HashMap::new(),
        }
    }

    /// Registers a factory function for a tool name.
    ///
    /// When `create()` is called for this tool name, the factory will be
    /// invoked to create the executor.
    #[must_use]
    pub fn with_executor<F>(mut self, tool_name: &str, factory: F) -> Self
    where
        F: Fn(ToolCallRequest) -> Box<dyn Executor> + Send + Sync + 'static,
    {
        self.factories
            .insert(tool_name.to_string(), Box::new(factory));
        self
    }
}

impl Default for TestExecutorSource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ExecutorSource for TestExecutorSource {
    async fn create(
        &self,
        request: ToolCallRequest,
        _tools_config: &ToolsConfig,
        _mcp_client: &Client,
    ) -> Result<Box<dyn Executor>, ToolError> {
        if let Some(factory) = self.factories.get(&request.name) {
            Ok(factory(request))
        } else {
            Err(ToolError::NotFound {
                name: request.name.clone(),
            })
        }
    }
}

/// Information needed to prompt for tool execution permission.
///
/// This struct contains all the data the `ToolPrompter` needs to show a
/// permission prompt to the user.
#[derive(Debug, Clone)]
pub struct PermissionInfo {
    /// The tool call ID.
    pub tool_id: String,

    /// The tool name.
    pub tool_name: String,

    /// The tool source (builtin, local, MCP).
    pub tool_source: ToolSource,

    /// The configured run mode.
    pub run_mode: RunMode,

    /// The arguments to pass to the tool.
    pub arguments: Value,
}
