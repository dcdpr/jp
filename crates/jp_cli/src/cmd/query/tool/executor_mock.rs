//! Executors that return a scripted result instead of running anything.

use std::{collections::HashMap, sync::Mutex};

use async_trait::async_trait;
use indexmap::IndexMap;
use jp_config::conversation::tool::ToolConfigWithDefaults;
use jp_conversation::event::{ToolCallRequest, ToolCallResponse};
use jp_mcp::server::{StderrSink, service::Formatted};
use jp_tool::{ToolDefinition, ToolDocs};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{Executor, ExecutorError, ExecutorResult, ExecutorSource, PermissionInfo};

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
/// let result = executor.execute(&answers, &client, &root, token, None).await;
/// ```
pub(crate) struct MockExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,
    permission_info: Option<PermissionInfo>,
    result: Mutex<Option<ExecutorResult>>,

    /// The diagnostic `prepare` rejects the call with, if it does.
    prepare_error: Option<String>,
}

impl MockExecutor {
    /// Creates a mock executor that returns a successful completion.
    pub(crate) fn completed(tool_id: &str, tool_name: &str, output: &str) -> Self {
        Self::new(tool_id, tool_name, Ok(output.to_owned()))
    }

    /// Creates a mock executor that returns an error.
    pub(crate) fn error(tool_id: &str, tool_name: &str, error: &str) -> Self {
        Self::new(tool_id, tool_name, Err(error.to_owned()))
    }

    fn new(tool_id: &str, tool_name: &str, result: Result<String, String>) -> Self {
        Self {
            tool_id: tool_id.to_owned(),
            tool_name: tool_name.to_owned(),
            arguments: Map::new(),
            permission_info: None,
            result: Mutex::new(Some(ExecutorResult::Completed(ToolCallResponse {
                id: tool_id.to_owned(),
                result,
            }))),
            prepare_error: None,
        }
    }

    /// Makes `prepare` reject the call with `message`, the way the execution
    /// service reports an argument formatter that failed.
    ///
    /// The call never reaches execution.
    pub(crate) fn rejected_at_prepare(mut self, message: &str) -> Self {
        self.prepare_error = Some(message.to_owned());
        self
    }

    /// Sets the arguments for this executor.
    pub(crate) fn with_arguments(mut self, args: Map<String, Value>) -> Self {
        self.arguments = args;
        self
    }

    /// Sets the permission info for this executor.
    ///
    /// If set, the executor will require permission prompting based on the
    /// configured `RunMode`.
    pub(crate) fn with_permission_info(mut self, info: PermissionInfo) -> Self {
        self.permission_info = Some(info);
        self
    }
}

#[async_trait]
impl Executor for MockExecutor {
    async fn prepare(
        &mut self,
        _render_arguments: bool,
    ) -> Result<Option<ToolCallResponse>, ExecutorError> {
        match &self.prepare_error {
            Some(message) => Err(ExecutorError::Rejected {
                message: message.clone(),
            }),
            None => Ok(None),
        }
    }

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
        // Arguments don't affect the pre-configured result.
    }

    async fn execute(
        &self,
        _answers: &IndexMap<String, Value>,
        _cancellation_token: CancellationToken,
        _stderr: Option<StderrSink>,
    ) -> ExecutorResult {
        self.result
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .unwrap_or_else(|| {
                ExecutorResult::Completed(ToolCallResponse {
                    id: self.tool_id.clone(),
                    result: Err("MockExecutor: result already consumed".to_owned()),
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
/// let coordinator = ToolCoordinator::new(tools_config, Box::new(source));
/// ```
#[derive(Default)]
pub(crate) struct TestExecutorSource {
    #[expect(
        clippy::type_complexity,
        reason = "A boxed factory per tool name, named inline rather than aliased once"
    )]
    factories: HashMap<String, Box<dyn Fn(ToolCallRequest) -> Box<dyn Executor> + Send + Sync>>,
}

impl TestExecutorSource {
    /// Creates a new empty test executor source.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers a factory function for a tool name.
    ///
    /// When `create()` is called for this tool name, the factory will be
    /// invoked to create the executor.
    pub(crate) fn with_executor<F>(mut self, tool_name: &str, factory: F) -> Self
    where
        F: Fn(ToolCallRequest) -> Box<dyn Executor> + Send + Sync + 'static,
    {
        self.factories
            .insert(tool_name.to_owned(), Box::new(factory));
        self
    }

    /// Returns stub [`ToolDefinition`]s for all registered tool names.
    ///
    /// Useful for passing to `run_turn_loop` so the availability check accepts
    /// the tools this source can handle.
    pub(crate) fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.factories
            .keys()
            .map(|name| ToolDefinition {
                name: name.clone(),
                docs: ToolDocs::default(),
                parameters: json!({ "type": "object", "properties": {} }),
                fan_out: false,
            })
            .collect()
    }
}

impl ExecutorSource for TestExecutorSource {
    fn create(
        &self,
        request: ToolCallRequest,
        _config: ToolConfigWithDefaults,
        op: Option<usize>,
    ) -> Option<Box<dyn Executor>> {
        let factory = self.factories.get(&request.name)?;
        let executor = factory(request);

        Some(match op {
            None => executor,
            Some(op) => Box::new(OpExecutor {
                inner: executor,
                op,
            }),
        })
    }
}

/// Wraps a test executor so it reports the operation it stands for.
///
/// Test factories build one executor from one request and know nothing about
/// fan-out; this carries the operation index the source was asked for without
/// every factory having to thread it through.
struct OpExecutor {
    inner: Box<dyn Executor>,
    op: usize,
}

#[async_trait]
impl Executor for OpExecutor {
    async fn prepare(
        &mut self,
        render_arguments: bool,
    ) -> Result<Option<ToolCallResponse>, ExecutorError> {
        self.inner.prepare(render_arguments).await
    }

    async fn approve(&mut self) -> Result<(), ExecutorError> {
        self.inner.approve().await
    }

    fn formatted_arguments(&self) -> Option<&Formatted> {
        self.inner.formatted_arguments()
    }

    fn tool_id(&self) -> &str {
        self.inner.tool_id()
    }

    fn tool_name(&self) -> &str {
        self.inner.tool_name()
    }

    fn op_index(&self) -> Option<usize> {
        Some(self.op)
    }

    fn arguments(&self) -> &Map<String, Value> {
        self.inner.arguments()
    }

    fn permission_info(&self) -> Option<PermissionInfo> {
        // The inner executor keys its info by the call alone; this one stands
        // for an operation of it.
        self.inner.permission_info().map(|info| PermissionInfo {
            state_key: self.state_key(),
            ..info
        })
    }

    fn needs_permission(&self) -> bool {
        self.inner.needs_permission()
    }

    fn set_arguments(&mut self, args: Value) {
        self.inner.set_arguments(args);
    }

    fn tool_failed(&self) -> bool {
        self.inner.tool_failed()
    }

    fn pause_for_restart(&self) -> bool {
        self.inner.pause_for_restart()
    }

    fn hold_for_response(&self) -> bool {
        self.inner.hold_for_response()
    }

    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        cancellation_token: CancellationToken,
        stderr: Option<StderrSink>,
    ) -> ExecutorResult {
        self.inner
            .execute(answers, cancellation_token, stderr)
            .await
    }
}
