//! Executors that return a scripted result instead of running anything.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use indexmap::IndexMap;
use jp_config::conversation::tool::ToolConfigWithDefaults;
use jp_conversation::event::{ToolCallRequest, ToolCallResponse};
use jp_mcp::server::StderrSink;
use jp_process::{ExitCode, MockProcessRunner, ProcessOutput, ProcessRunner};
use jp_tool::{Outcome, Question, ToolDefinition, ToolDocs};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::{Executor, ExecutorResult, ExecutorSource, PermissionInfo};

/// A runner for a service whose tools run no local command.
///
/// Running one returns an error naming it.
pub(crate) fn no_commands() -> Arc<dyn ProcessRunner> {
    Arc::new(MockProcessRunner::never_called())
}

/// The `style.parameters` of a formatter that asks the tool's `confirm`
/// question, and describes the call as `confirm = <answer>` once it has one.
///
/// Pair it with [`asking_formatter_runner`], which plays the formatter.
/// The template is rendered for real, so the answer reaches the formatter the
/// way it reaches any command: through `{{tool.answers.confirm}}`.
pub(crate) fn asking_formatter() -> Value {
    json!({"program": "formatter", "args": ["{{tool.answers.confirm}}"], "shell": false})
}

/// Plays the formatter [`asking_formatter`] configures.
///
/// An unanswered question renders as `null`, which the formatter meets by
/// asking `question`, in the wire format a real tool prints.
pub(crate) fn asking_formatter_runner(question: &str) -> Arc<dyn ProcessRunner> {
    let asking = serde_json::to_string(&Outcome::NeedsInput {
        question: Question::boolean("confirm", question).unwrap(),
    })
    .unwrap();

    Arc::new(MockProcessRunner::responding(move |spec| {
        let stdout = match spec.args.as_slice() {
            [answer] if answer != "null" => format!("confirm = {answer}"),
            _ => asking.clone(),
        };
        Ok(ProcessOutput {
            stdout,
            stderr: String::new(),
            status: ExitCode::success(),
        })
    }))
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
/// let result = executor.execute(&answers, &client, &root, token, None).await;
/// ```
pub(crate) struct MockExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,
    permission_info: Option<PermissionInfo>,
    result: Mutex<Option<ExecutorResult>>,
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
        }
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
    fn tool_id(&self) -> &str {
        &self.tool_id
    }

    fn tool_name(&self) -> &str {
        &self.tool_name
    }

    fn arguments(&self) -> Map<String, Value> {
        self.arguments.clone()
    }

    fn permission_info(&self) -> Option<PermissionInfo> {
        self.permission_info.clone()
    }

    fn set_arguments(&self, _args: Value) {
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
            })
            .collect()
    }
}

impl ExecutorSource for TestExecutorSource {
    fn create(
        &self,
        request: ToolCallRequest,
        _config: ToolConfigWithDefaults,
    ) -> Option<Box<dyn Executor>> {
        let factory = self.factories.get(&request.name)?;
        Some(factory(request))
    }
}
