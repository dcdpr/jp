//! Per-call execution and private MCP Host interactions.
//!
//! [`Service::start_call`] is the execution entry point for the MCP handler.
//! Calls run independently of their result receivers.
//! Dropping a receiver does not cancel or retry work; use [`Call::cancel`] or
//! [`Service::cancel_current`].
//! The MCP Host must drain [`HostReceiver`] while calls are outstanding.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use camino::Utf8PathBuf;
use indexmap::IndexMap;
use jp_config::conversation::tool::{
    FormatMode, ResultMode, RunMode, ToolConfigWithDefaults, ToolSource, style::ParametersStyle,
};
use jp_tool::{
    AccessPolicy, Action, ContentBlock, Error as ToolError, InputRequest, ToolDefinition,
    definition::{apply_parameter_defaults, validate_tool_arguments},
    schema::Node,
};
use serde_json::{Map, Value};
use tokio::sync::{Notify, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{
    CommandResult, ExecutionOutcome, InvocationContext, builtin::BuiltinExecutors, execute,
    run_tool_command, tool_context,
};
use crate::{CallToolResult, Client, Content};

/// A tool resolved under trusted MCP Host configuration.
#[derive(Clone, Debug)]
pub struct ConfiguredTool {
    /// The name and source-neutral argument schema advertised to callers.
    pub definition: ToolDefinition,
    /// Execution and interaction requirements, including source selection.
    pub config: ToolConfigWithDefaults,
    /// Compiled access grants supplied by the MCP Host, never by an MCP caller.
    /// A compilation failure is delivered as a tool error without execution.
    pub access: Result<Option<AccessPolicy>, String>,
}

/// An invocation received by the MCP handler.
/// Contains no execution authority.
#[derive(Clone, Debug)]
pub struct CallRequest {
    /// The advertised tool name, not the upstream implementation name.
    pub name: String,
    /// Arguments supplied by the caller.
    pub arguments: Map<String, Value>,
    /// Opaque caller metadata for Host-side correlation only.
    pub correlation: Map<String, Value>,
}

/// Service-assigned identity, distinct from caller-supplied protocol IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvocationId(u64);

/// Identity and original input accompanying each private Host interaction.
#[derive(Clone, Debug)]
pub struct CallInfo {
    /// The service-assigned invocation ID.
    pub id: InvocationId,
    /// Original caller input; edits do not change it.
    pub request: CallRequest,
}

/// A required interaction for one invocation.
///
/// Replies are single-use and bound to this request.
/// Secret answers and accumulated input are deliberately not exposed through a
/// `Debug` impl.
pub struct HostRequest {
    /// Identity and requested arguments for recording/correlation.
    pub call: CallInfo,
    /// The operation the MCP Host must complete.
    pub interaction: Interaction,
}

/// Receiver held exclusively by the MCP Host.
pub type HostReceiver = mpsc::Receiver<HostRequest>;

/// A Host reply that may fail, for example when recording could not complete.
pub type HostReply<T> = Result<T, HostError>;

/// A failure reported by the MCP Host, without a persisted conversation type.
#[derive(Debug, Clone, thiserror::Error)]
#[error("MCP Host operation failed: {0}")]
pub struct HostError(pub String);

/// Whether the Host approved execution, and which edited arguments to use.
#[derive(Debug)]
pub enum Admission {
    /// Approved arguments.
    /// They are validated again before release.
    Run { arguments: Map<String, Value> },
    /// Do not execute.
    /// Deliver and record this explanation.
    Skip { reason: String },
    /// Resolve a call without execution, preserving an error response if
    /// needed.
    Complete { result: Result<String, String> },
}

/// The Host may answer a question or resolve the call without another attempt.
#[derive(Debug)]
pub enum InputAnswer {
    /// Validated by the service before another execution attempt.
    Answer(Value),
    /// A declined or cancelled inquiry resolves the logical call.
    Complete { result: Result<String, String> },
}

impl From<Value> for InputAnswer {
    fn from(value: Value) -> Self {
        Self::Answer(value)
    }
}

/// The Host releases a prepared call or resolves it without execution.
#[derive(Debug)]
pub enum ReleaseDecision {
    /// Begin execution with the approved arguments.
    Execute,
    /// Preparation failed or the Host stopped the call before execution.
    Complete { result: Result<String, String> },
}

/// Host-only services needed by the per-call execution state machine.
pub enum Interaction {
    /// Ask whether argument presentation is wanted.
    /// This does not authorize an approval-gated formatter to run early.
    RenderArguments {
        /// True when the MCP Host needs the custom representation.
        reply: oneshot::Sender<HostReply<bool>>,
    },
    /// Apply admission policy and argument editing.
    /// A successful reply also acknowledges recording/preparation of the
    /// request.
    Prepare {
        /// Resolved interaction requirements, including formatter policy.
        config: Box<ToolConfigWithDefaults>,
        /// Coerced/defaulted arguments presented for approval.
        arguments: Map<String, Value>,
        /// Custom formatter output, if formatting was permitted before
        /// approval.
        formatted_arguments: Option<Result<String, String>>,
        /// One reply for this preparation operation.
        reply: oneshot::Sender<HostReply<Admission>>,
    },
    /// Wait for the Host's execution phase and recording barrier.
    Release {
        /// Validated arguments that will actually execute.
        arguments: Map<String, Value>,
        /// Custom representation of the approved arguments, if requested.
        formatted_arguments: Option<Result<String, String>>,
        /// Permission to execute, or a final response without execution.
        reply: oneshot::Sender<HostReply<ReleaseDecision>>,
    },
    /// Obtain and record input before the next execution attempt.
    Input {
        /// The expected answer shape and secrecy constraints.
        request: InputRequest,
        /// Context shown with the input request.
        supporting: Vec<ContentBlock>,
        /// Accumulated answers.
        /// These may contain secrets and must not be logged.
        answers: IndexMap<String, Value>,
        /// The answer, after Host routing and recording/redaction.
        reply: oneshot::Sender<HostReply<InputAnswer>>,
    },
    /// Review/edit a completed result under the configured delivery policy.
    Review {
        /// The required delivery interaction.
        mode: ResultMode,
        /// Unedited execution result.
        result: Result<String, String>,
        /// The content approved for delivery, including skip explanations.
        reply: oneshot::Sender<HostReply<Result<String, String>>>,
    },
    /// Acknowledge final recording before returning the result to the caller.
    Record {
        /// Post-edit execution arguments, separate from `CallInfo::request`.
        arguments: Map<String, Value>,
        /// Original completed result; absent for skipped calls.
        raw_result: Option<Result<String, String>>,
        /// Content approved for delivery.
        result: Result<String, String>,
        /// Acknowledges the Host's configured persistence policy, not an
        /// unconditional disk write.
        reply: oneshot::Sender<HostReply<()>>,
    },
}

/// Bounded, best-effort progress.
/// It is independent of required Host requests.
#[derive(Clone, Debug)]
pub struct Progress {
    /// Invocation emitting the line.
    pub id: InvocationId,
    /// A tool stderr line, without its newline terminator.
    pub line: String,
}

/// Failure of the service protocol or execution infrastructure.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    /// The service no longer admits calls.
    #[error("JP MCP Server is stopped")]
    Stopped,
    /// Work was explicitly cancelled before delivery.
    #[error("Tool invocation cancelled")]
    Cancelled,
    /// The required Host interaction connection was lost.
    #[error("MCP Host disconnected before completing the interaction")]
    HostDisconnected,
    /// The Host declined an operation, including failed recording.
    #[error(transparent)]
    Host(#[from] HostError),
    /// Tool lookup, validation, or execution failed.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// The Host returned data outside the tool's requested answer shape.
    #[error("Invalid answer for tool question `{0}`")]
    InvalidAnswer(String),
    /// An argument violates the schema's type or enumeration.
    #[error("Invalid tool argument at `{path}`: value violates its type or enum")]
    InvalidArgument { path: String },
    /// A configured name cannot select multiple tool implementations.
    #[error("Duplicate tool configured: {0}")]
    DuplicateTool(String),
    /// Restricted configuration must have a compiled policy.
    #[error("Missing compiled access policy for tool `{0}`")]
    MissingAccessPolicy(String),
    /// IDs must not wrap and alias an earlier invocation.
    #[error("Tool invocation identifiers exhausted")]
    IdExhausted,
    /// An execution task failed without producing a result.
    #[error("Tool execution task ended without a result")]
    TaskLost,
}

/// Handle to a submitted call.
/// Dropping it leaves execution running.
#[derive(Debug)]
pub struct Call {
    id: InvocationId,
    cancellation: CancellationToken,
    result: oneshot::Receiver<Result<CallOutput, ServiceError>>,
}

impl Call {
    /// Service identity to correlate with Host interactions.
    #[must_use]
    pub fn id(&self) -> InvocationId {
        self.id
    }

    /// Whether the final result or task failure is ready to receive.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        !self.result.is_empty() || self.result.is_terminated()
    }

    pub(super) fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    /// Cancel this invocation, including a pending Host interaction.
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Wait for execution and the final Host recording acknowledgement.
    pub async fn finish(self) -> Result<Result<String, String>, ServiceError> {
        self.result
            .await
            .map_err(|_| ServiceError::TaskLost)?
            .map(|output| output.text)
    }
}

#[derive(Debug)]
struct CallOutput {
    text: Result<String, String>,
    native: Option<CallToolResult>,
    delivery_decided: bool,
}

impl Call {
    /// Receive the complete MCP result, retaining unedited upstream content.
    pub async fn finish_mcp(self) -> Result<CallToolResult, ServiceError> {
        let output = self.result.await.map_err(|_| ServiceError::TaskLost)??;
        Ok(output.native.unwrap_or_else(|| match output.text {
            Ok(text) => CallToolResult::success(vec![Content::text(text)]),
            Err(text) => CallToolResult::error(vec![Content::text(text)]),
        }))
    }
}

/// In-process tool service with immutable Host-bound execution context.
///
/// The upstream client must be owned by this service: shutdown closes its
/// services, including connections visible through any clones of that client.
/// Dropping this owner signals cancellation; [`shutdown`] additionally waits
/// for cleanup.
///
/// [`shutdown`]: Self::shutdown
pub struct Service {
    inner: Arc<Inner>,
}

struct Inner {
    tools: IndexMap<String, ConfiguredTool>,
    upstream: Client,
    builtins: BuiltinExecutors,
    root: Utf8PathBuf,
    invocation: InvocationContext,
    host: mpsc::Sender<HostRequest>,
    progress: broadcast::Sender<Progress>,
    state: Mutex<State>,
    idle: Notify,
}

#[derive(Default)]
struct State {
    stopped: bool,
    next_id: u64,
    active: HashMap<InvocationId, CancellationToken>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        // No caller code runs under this lock. Recovering it permits cleanup
        // after an unrelated panic rather than orphaning active calls.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

struct ActiveCall {
    inner: Arc<Inner>,
    id: InvocationId,
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        self.inner.state().active.remove(&self.id);
        self.inner.idle.notify_waiters();
    }
}

impl Service {
    /// Bind resolved tools, access policies, and working context to the
    /// service.
    ///
    /// No tool code is run.
    /// The returned receiver is the private Host interface.
    pub fn new(
        tools: Vec<ConfiguredTool>,
        upstream: Client,
        builtins: BuiltinExecutors,
        root: Utf8PathBuf,
        invocation: InvocationContext,
    ) -> Result<(Self, HostReceiver), ServiceError> {
        let mut catalog = IndexMap::new();
        for tool in tools {
            if tool.config.access().is_some() && matches!(tool.access, Ok(None)) {
                return Err(ServiceError::MissingAccessPolicy(tool.definition.name));
            }
            let name = tool.definition.name.clone();
            if catalog.insert(name.clone(), tool).is_some() {
                return Err(ServiceError::DuplicateTool(name));
            }
        }
        let (host, receiver) = mpsc::channel(32);
        let (progress, _) = broadcast::channel(64);
        Ok((
            Self {
                inner: Arc::new(Inner {
                    tools: catalog,
                    upstream,
                    builtins,
                    root,
                    invocation,
                    host,
                    progress,
                    state: Mutex::new(State::default()),
                    idle: Notify::new(),
                }),
            },
            receiver,
        ))
    }

    /// Advertised definitions in their configured order.
    pub fn definitions(&self) -> impl Iterator<Item = &ToolDefinition> {
        self.inner.tools.values().map(|tool| &tool.definition)
    }

    /// Subscribe to stderr progress without slowing execution or Host replies.
    /// A lagging subscriber receives the broadcast channel's lag error.
    #[must_use]
    pub fn subscribe_progress(&self) -> broadcast::Receiver<Progress> {
        self.inner.progress.subscribe()
    }

    /// Submit work from the MCP handler and allocate an independent invocation
    /// ID.
    ///
    /// The caller must be inside a Tokio runtime.
    /// Caller metadata is forwarded only for correlation; it never sets the
    /// root, policy, or answers.
    pub fn start_call(&self, request: CallRequest) -> Result<Call, ServiceError> {
        let inner = self.inner.clone();
        let mut state = inner.state();
        if state.stopped {
            return Err(ServiceError::Stopped);
        }
        let tool = inner
            .tools
            .get(&request.name)
            .cloned()
            .ok_or_else(|| ToolError::NotFound {
                name: request.name.clone(),
            })?;
        state.next_id = state
            .next_id
            .checked_add(1)
            .ok_or(ServiceError::IdExhausted)?;
        let id = InvocationId(state.next_id);
        let cancellation = CancellationToken::new();
        state.active.insert(id, cancellation.clone());
        drop(state);
        let (sender, result) = oneshot::channel();
        let task_token = cancellation.clone();
        let active = ActiveCall {
            inner: inner.clone(),
            id,
        };
        tokio::spawn(async move {
            let _active = active;
            let call = CallInfo { id, request };
            let result = tokio::select! {
                biased;
                () = task_token.cancelled() => Err(ServiceError::Cancelled),
                () = inner.host.closed() => Err(ServiceError::HostDisconnected),
                result = run_call(&inner, &call, tool, &task_token) => result,
            };
            drop(sender.send(result));
        });
        Ok(Call {
            id,
            cancellation,
            result,
        })
    }

    /// Cancel an invocation identified through the private Host channel.
    pub fn cancel_call(&self, id: InvocationId) {
        if let Some(token) = self.inner.state().active.get(&id) {
            token.cancel();
        }
    }

    /// Stop current calls without preventing admission of later work.
    pub fn cancel_current(&self) {
        for token in self.inner.state().active.values() {
            token.cancel();
        }
    }

    /// Stop admission and signal cancellation without waiting for cleanup.
    pub fn stop(&self) {
        let mut state = self.inner.state();
        state.stopped = true;
        for token in state.active.values() {
            token.cancel();
        }
    }

    /// Stop admission, cancel outstanding calls, wait for their cleanup, and
    /// close owned upstream services.
    /// Safe to call more than once.
    pub async fn shutdown(&self) -> Result<(), ServiceError> {
        self.stop();
        loop {
            let idle = self.inner.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.inner.state().active.is_empty() {
                break;
            }
            idle.await;
        }
        self.inner.upstream.shutdown().await;
        Ok(())
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn ask<T>(
    inner: &Inner,
    call: &CallInfo,
    interaction: impl FnOnce(oneshot::Sender<HostReply<T>>) -> Interaction,
) -> Result<T, ServiceError> {
    let (reply, receiver) = oneshot::channel();
    inner
        .host
        .send(HostRequest {
            call: call.clone(),
            interaction: interaction(reply),
        })
        .await
        .map_err(|_| ServiceError::HostDisconnected)?;
    receiver
        .await
        .map_err(|_| ServiceError::HostDisconnected)?
        .map_err(Into::into)
}

fn validate_arguments(
    tool: &ConfiguredTool,
    arguments: &mut Map<String, Value>,
) -> Result<(), ServiceError> {
    tool.definition.coerce_arguments(arguments);
    apply_parameter_defaults(arguments, &tool.definition.parameters);
    validate_tool_arguments(arguments, &tool.definition.parameters)?;
    for (name, node) in Node::root(&tool.definition.parameters).properties() {
        if let Some(value) = arguments.get(&name) {
            validate_value(&name, value, &node)?;
        }
    }
    Ok(())
}

fn validate_value(path: &str, value: &Value, node: &Node<'_>) -> Result<(), ServiceError> {
    if !node.permits(value) {
        return Err(ServiceError::InvalidArgument { path: path.into() });
    }
    if let Some(object) = value.as_object() {
        for (name, child) in node.properties() {
            if let Some(value) = object.get(&name) {
                validate_value(&format!("{path}.{name}"), value, &child)?;
            }
        }
    }
    if let (Some(values), Some(items)) = (value.as_array(), node.items()) {
        for (index, value) in values.iter().enumerate() {
            validate_value(&format!("{path}[{index}]"), value, &items)?;
        }
    }
    Ok(())
}

async fn run_call(
    inner: &Inner,
    call: &CallInfo,
    tool: ConfiguredTool,
    cancellation: &CancellationToken,
) -> Result<CallOutput, ServiceError> {
    let mut arguments = call.request.arguments.clone();
    validate_arguments(&tool, &mut arguments)?;
    let wants_format = if tool.config.run() != RunMode::Skip
        && !tool.config.style().hidden
        && matches!(tool.config.style().parameters, ParametersStyle::Custom(_))
    {
        ask(inner, call, |reply| Interaction::RenderArguments { reply }).await?
    } else {
        false
    };
    let mut formatted_arguments = if wants_format && tool.config.format() == FormatMode::Unattended
    {
        Some(format_arguments(inner, &tool, &arguments, cancellation).await?)
    } else {
        None
    };
    let original_arguments = arguments.clone();
    let admission = if tool.config.run() == RunMode::Skip {
        Admission::Skip {
            reason: "Tool execution skipped by configuration.".into(),
        }
    } else {
        ask(inner, call, |reply| Interaction::Prepare {
            config: Box::new(tool.config.clone()),
            arguments: arguments.clone(),
            formatted_arguments: formatted_arguments.clone(),
            reply,
        })
        .await?
    };
    let admission = match admission {
        Admission::Skip { reason } => Admission::Complete { result: Ok(reason) },
        other => other,
    };
    arguments = match admission {
        Admission::Run { arguments } => arguments,
        Admission::Complete { result } => {
            ask(inner, call, |reply| Interaction::Record {
                arguments,
                raw_result: None,
                result: result.clone(),
                reply,
            })
            .await?;
            return Ok(CallOutput {
                text: result,
                native: None,
                delivery_decided: true,
            });
        }
        Admission::Skip { .. } => unreachable!("skip was normalized above"),
    };
    validate_arguments(&tool, &mut arguments)?;
    if wants_format && (formatted_arguments.is_none() || arguments != original_arguments) {
        formatted_arguments = Some(format_arguments(inner, &tool, &arguments, cancellation).await?);
    }
    let release = ask(inner, call, |reply| Interaction::Release {
        arguments: arguments.clone(),
        formatted_arguments,
        reply,
    })
    .await?;
    let (output, executed) = match release {
        ReleaseDecision::Execute => (
            execute_with_answers(inner, call, &tool, &arguments, cancellation).await?,
            true,
        ),
        ReleaseDecision::Complete { result } => (
            CallOutput {
                text: result,
                native: None,
                delivery_decided: true,
            },
            false,
        ),
    };
    deliver_result(inner, call, &tool, arguments, output, executed).await
}

async fn deliver_result(
    inner: &Inner,
    call: &CallInfo,
    tool: &ConfiguredTool,
    arguments: Map<String, Value>,
    output: CallOutput,
    executed: bool,
) -> Result<CallOutput, ServiceError> {
    let CallOutput {
        text: raw_result,
        native,
        delivery_decided,
    } = output;
    let result = if delivery_decided {
        raw_result.clone()
    } else {
        match tool.config.result() {
            ResultMode::Skip => Ok("Result delivery skipped by configuration.".into()),
            ResultMode::Unattended => raw_result.clone(),
            mode @ (ResultMode::Ask | ResultMode::Edit) => {
                ask(inner, call, |reply| Interaction::Review {
                    mode,
                    result: raw_result.clone(),
                    reply,
                })
                .await?
            }
        }
    };
    let native =
        native.filter(|_| result == raw_result && tool.config.result() != ResultMode::Skip);
    ask(inner, call, |reply| Interaction::Record {
        arguments,
        raw_result: (executed && !delivery_decided).then_some(raw_result),
        result: result.clone(),
        reply,
    })
    .await?;
    Ok(CallOutput {
        text: result,
        native,
        delivery_decided: true,
    })
}

async fn execute_with_answers(
    inner: &Inner,
    call: &CallInfo,
    tool: &ConfiguredTool,
    arguments: &Map<String, Value>,
    cancellation: &CancellationToken,
) -> Result<CallOutput, ServiceError> {
    let access = match &tool.access {
        Ok(access) => access.as_ref(),
        Err(error) => {
            return Ok(CallOutput {
                text: Err(error.clone()),
                native: None,
                delivery_decided: false,
            });
        }
    };
    let mut answers = IndexMap::new();
    loop {
        let progress = inner.progress.clone();
        let id = call.id;
        let stderr = Arc::new(move |line: &str| {
            drop(progress.send(Progress {
                id,
                line: line.into(),
            }));
        });
        let outcome = execute(
            &tool.definition,
            call.id.0.to_string(),
            Value::Object(arguments.clone()),
            &answers,
            &tool.config,
            &inner.upstream,
            &inner.root,
            cancellation.clone(),
            &inner.builtins,
            access,
            &inner.invocation,
            Some(stderr),
        )
        .await?;
        match outcome {
            ExecutionOutcome::Cancelled { .. } => return Err(ServiceError::Cancelled),
            ExecutionOutcome::Completed { result, native, .. } => {
                return Ok(CallOutput {
                    text: result,
                    native,
                    delivery_decided: false,
                });
            }
            ExecutionOutcome::NeedsInput { mut question, .. } => {
                let supporting = question
                    .pre_amble
                    .take()
                    .into_iter()
                    .map(ContentBlock::text)
                    .collect();
                let request = InputRequest::from(question);
                let answer = ask(inner, call, |reply| Interaction::Input {
                    request: request.clone(),
                    supporting,
                    answers: answers.clone(),
                    reply,
                })
                .await?;
                let answer = match answer {
                    InputAnswer::Answer(answer) => answer,
                    InputAnswer::Complete { result } => {
                        return Ok(CallOutput {
                            text: result,
                            native: None,
                            delivery_decided: true,
                        });
                    }
                };
                if !Node::root(&Value::Object(request.schema)).permits(&answer) {
                    return Err(ServiceError::InvalidAnswer(request.id.to_string()));
                }
                answers.insert(request.id.to_string(), answer);
            }
        }
    }
}

async fn format_arguments(
    inner: &Inner,
    tool: &ConfiguredTool,
    arguments: &Map<String, Value>,
    cancellation: &CancellationToken,
) -> Result<Result<String, String>, ServiceError> {
    let ParametersStyle::Custom(command) = &tool.config.style().parameters else {
        return Ok(Ok(String::new()));
    };
    let name = match tool.config.source() {
        ToolSource::Local { tool: name }
        | ToolSource::Builtin { tool: name }
        | ToolSource::Mcp { tool: name, .. } => name.as_deref().unwrap_or(&tool.definition.name),
    };
    let context = tool_context(
        name,
        &Value::Object(arguments.clone()),
        &IndexMap::new(),
        &tool.config,
        &inner.root,
        &Action::FormatArguments,
        tool.access
            .as_ref()
            .map_err(|error| ServiceError::Host(HostError(error.clone())))?
            .as_ref(),
        &inner.invocation,
    );
    let result = match run_tool_command(
        command.clone().command(),
        context,
        &inner.root,
        cancellation.clone(),
        None,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => return Ok(Err(error.to_string())),
    };
    match result {
        CommandResult::NeedsInput(_) => {
            Ok(Err("Custom arguments formatter requested input.".into()))
        }
        CommandResult::Cancelled => Err(ServiceError::Cancelled),
        CommandResult::Success(text) => Ok(Ok(text.trim().into())),
        CommandResult::TransientError { message, trace } => {
            Ok(Err(CommandResult::format_error(&message, &trace)))
        }
        other => Ok(other.into_tool_result(name).map(|text| text.trim().into())),
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
