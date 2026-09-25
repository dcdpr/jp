//! Per-call execution and private MCP Host interactions.
//!
//! [`Service::start_call`] is the execution entry point for the MCP handler.
//! Calls run independently of their result receivers.
//! Dropping a receiver does not cancel or retry work; use [`Call::cancel`] or
//! [`Service::cancel_current`].
//! The MCP Host must drain [`HostReceiver`] while calls are outstanding.

use std::{
    collections::HashMap,
    error::Error as StdError,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use camino::Utf8PathBuf;
use indexmap::IndexMap;
use jp_config::conversation::tool::{
    FormatMode, ResultMode, RunMode, ToolConfigWithDefaults, style::ParametersStyle,
};
use jp_process::ProcessRunner;
use jp_tool::{
    AccessPolicy, Action, ContentBlock, Error as ToolError, InputRequest, Question, QuestionId,
    ToolDefinition, ToolResult,
    definition::{apply_parameter_defaults, validate_tool_arguments},
    schema::Node,
};
use serde_json::{Map, Value};
use tokio::sync::{Notify, broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::{
    Answers, CommandResult, Execution, ExecutionOutcome, InvocationContext, StderrSink,
    builtin::BuiltinExecutors,
    execute,
    result::{ResultError, to_mcp},
};
use crate::{CallToolResult, Client};

/// A tool resolved under trusted MCP Host configuration.
#[derive(Clone, Debug)]
pub struct ConfiguredTool {
    /// The name and source-neutral argument schema advertised to callers.
    pub definition: ToolDefinition,
    /// Execution and interaction requirements, including source selection.
    pub config: ToolConfigWithDefaults,
    /// Compiled access grants supplied by the MCP Host, never by an MCP caller.
    /// A compilation failure is delivered as a tool error without execution.
    pub access: Result<Option<AccessPolicy>, AccessPolicyError>,
    /// Opaque Host-supplied metadata advertised on this tool's MCP description.
    /// It does not change execution policy or interpret vendor-specific hints.
    pub metadata: Map<String, Value>,
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
pub enum HostError {
    /// The Host could not record the event under its persistence policy.
    #[error("MCP Host operation failed: {0}")]
    Recording(#[source] Arc<dyn StdError + Send + Sync>),
}

/// Compilation failed before a call could obtain its access policy.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid access policy for tool '{tool}': {source}")]
pub struct AccessPolicyError {
    /// The configured tool whose policy failed.
    pub tool: String,
    /// The original compiler error, retained for diagnostics.
    #[source]
    pub source: Arc<dyn StdError + Send + Sync>,
}

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
    Complete { result: ToolResult },
}

/// The Host may answer a question or resolve the call without another attempt.
#[derive(Debug)]
pub enum InputAnswer {
    /// Validated by the service before another execution attempt.
    Answer(Value),
    /// A declined or cancelled inquiry resolves the logical call.
    Complete { result: ToolResult },
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
    Complete { result: ToolResult },
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
        /// The argument formatter's description of the call, if formatting was
        /// permitted before approval.
        formatted_arguments: Option<String>,
        /// One reply for this preparation operation.
        reply: oneshot::Sender<HostReply<Admission>>,
    },
    /// Wait for the Host's execution phase and recording barrier.
    Release {
        /// Validated arguments that will actually execute.
        arguments: Map<String, Value>,
        /// The argument formatter's description of the approved arguments, if
        /// requested.
        formatted_arguments: Option<String>,
        /// Permission to execute, or a final response without execution.
        reply: oneshot::Sender<HostReply<ReleaseDecision>>,
    },
    /// Obtain and record input before the tool or its argument formatter runs
    /// again.
    ///
    /// A formatter's question arrives before the call's [`Prepare`], or, for a
    /// formatter held back until admission, before its [`Release`].
    /// Every answer given is also given to the tool when it runs, so the call
    /// that executes is the one the formatter described.
    ///
    /// [`Prepare`]: Self::Prepare
    /// [`Release`]: Self::Release
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
        result: ToolResult,
        /// The content approved for delivery, including skip explanations.
        reply: oneshot::Sender<HostReply<ToolResult>>,
    },
    /// Acknowledge final recording before returning the result to the caller.
    Record {
        /// What the Host is being asked to record.
        ///
        /// Boxed because it is the largest thing the private channel carries,
        /// and every other interaction in flight would otherwise be sized for
        /// it.
        recording: Box<Recording>,
        /// Acknowledges the Host's configured persistence policy, not an
        /// unconditional disk write.
        reply: oneshot::Sender<HostReply<()>>,
    },
}

/// One call as the Host should record it.
#[derive(Debug)]
pub struct Recording {
    /// Post-edit execution arguments, separate from `CallInfo::request`.
    pub arguments: Map<String, Value>,

    /// Original completed result; absent for skipped calls.
    pub raw_result: Option<ToolResult>,

    /// Content approved for delivery.
    pub result: ToolResult,
}

impl Interaction {
    /// Whether the server has abandoned this interaction, for example after a
    /// restart.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        match self {
            Self::RenderArguments { reply } => reply.is_closed(),
            Self::Prepare { reply, .. } => reply.is_closed(),
            Self::Release { reply, .. } => reply.is_closed(),
            Self::Input { reply, .. } => reply.is_closed(),
            Self::Review { reply, .. } => reply.is_closed(),
            Self::Record { reply, .. } => reply.is_closed(),
        }
    }
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
    /// Access policy compilation failed before formatting or execution.
    #[error(transparent)]
    Access(#[from] AccessPolicyError),
    /// A final result cannot be represented by the MCP transport.
    #[error(transparent)]
    Result(#[from] ResultError),
    /// Tool lookup, validation, or execution failed.
    #[error(transparent)]
    Tool(#[from] ToolError),
    /// The Host returned data outside the tool's requested answer shape.
    #[error("Invalid answer for tool question `{0}`")]
    InvalidAnswer(QuestionId),
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
    pub async fn finish(self) -> Result<ToolResult, ServiceError> {
        self.result
            .await
            .map_err(|_| ServiceError::TaskLost)?
            .map(|output| output.result)
    }
}

#[derive(Debug)]
struct CallOutput {
    result: ToolResult,
    delivery_decided: bool,
}

impl Call {
    /// Receive the complete MCP result, retaining unedited upstream content.
    pub async fn finish_mcp(self) -> Result<CallToolResult, ServiceError> {
        let output = self.result.await.map_err(|_| ServiceError::TaskLost)??;
        Ok(to_mcp(output.result)?)
    }
}

/// In-process tool service with immutable Host-bound execution context.
///
/// The upstream client is borrowed from the MCP Host, which may share its
/// connections with other services running concurrent Turns, so neither
/// dropping nor shutting down the service closes them.
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
    runner: Arc<dyn ProcessRunner>,
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
    active: HashMap<InvocationId, CallControl>,
}

struct CallControl {
    lifetime: CancellationToken,
    attempt: CancellationToken,
    resume: Arc<Notify>,

    /// The result the Host resolved this call with, delivered in place of
    /// another attempt when the paused call wakes.
    completion: Option<ToolResult>,
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
    /// `runner` runs local commands: `local` tools, and every tool's argument
    /// formatter.
    /// The returned receiver is the private Host interface.
    pub fn new(
        tools: Vec<ConfiguredTool>,
        upstream: Client,
        builtins: BuiltinExecutors,
        runner: Arc<dyn ProcessRunner>,
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
                    runner,
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

    /// Metadata supplied by the Host; empty maps are omitted from descriptions.
    pub(super) fn tool_metadata(&self, name: &str) -> Option<&Map<String, Value>> {
        self.inner
            .tools
            .get(name)
            .map(|tool| &tool.metadata)
            .filter(|meta| !meta.is_empty())
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
        let mut attempt = cancellation.child_token();
        let resume = Arc::new(Notify::new());
        state.active.insert(id, CallControl {
            lifetime: cancellation.clone(),
            attempt: attempt.clone(),
            resume: resume.clone(),
            completion: None,
        });
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
            let result = loop {
                let result = tokio::select! {
                    biased;
                    () = task_token.cancelled() => Some(Err(ServiceError::Cancelled)),
                    () = inner.host.closed() => Some(Err(ServiceError::HostDisconnected)),
                    () = attempt.cancelled() => None,
                    result = run_call(&inner, &call, tool.clone(), &attempt) => Some(result),
                };
                if let Some(result) = result {
                    break result;
                }
                // The MCP caller still owns the same pending request. Wait for
                // the Host to re-prepare it before opening another attempt.
                tokio::select! {
                    biased;
                    () = task_token.cancelled() => break Err(ServiceError::Cancelled),
                    () = inner.host.closed() => break Err(ServiceError::HostDisconnected),
                    () = resume.notified() => {},
                }
                let completion = inner
                    .state()
                    .active
                    .get_mut(&id)
                    .and_then(|control| control.completion.take());
                if let Some(result) = completion {
                    break Ok(CallOutput {
                        result,
                        delivery_decided: true,
                    });
                }
                attempt = task_token.child_token();
                if let Some(control) = inner.state().active.get_mut(&id) {
                    control.attempt = attempt.clone();
                }
            };
            drop(sender.send(result));
        });
        Ok(Call {
            id,
            cancellation,
            result,
        })
    }

    /// Observe cancellation of an invocation through the private Host channel.
    /// Returns `None` after the invocation has left the active set.
    #[must_use]
    pub fn call_cancellation(&self, id: InvocationId) -> Option<CancellationToken> {
        self.inner
            .state()
            .active
            .get(&id)
            .map(|control| control.lifetime.clone())
    }

    /// Stop the current attempt without completing the MCP call.
    /// The Host must call `resume_call` to re-prepare and release another
    /// attempt.
    #[must_use]
    pub fn pause_call(&self, id: InvocationId) -> bool {
        let state = self.inner.state();
        let Some(control) = state.active.get(&id) else {
            return false;
        };
        control.attempt.cancel();
        true
    }

    /// Allow a paused call to start another preparation/approval cycle.
    pub fn resume_call(&self, id: InvocationId) {
        if let Some(control) = self.inner.state().active.get(&id) {
            control.resume.notify_one();
        }
    }

    /// Resolve an invocation with `result` in place of its current attempt.
    ///
    /// The attempt stops, and `result` is what the MCP caller receives.
    /// No review or recording interaction follows: the Host supplies a result
    /// it has already recorded.
    ///
    /// Returns `false` when the invocation has left the active set, in which
    /// case its caller already has a result.
    #[must_use]
    pub fn complete_call(&self, id: InvocationId, result: ToolResult) -> bool {
        let mut state = self.inner.state();
        let Some(control) = state.active.get_mut(&id) else {
            return false;
        };
        control.completion = Some(result);
        control.attempt.cancel();
        control.resume.notify_one();
        true
    }

    /// Cancel an invocation identified through the private Host channel.
    pub fn cancel_call(&self, id: InvocationId) {
        if let Some(token) = self.inner.state().active.get(&id) {
            token.lifetime.cancel();
        }
    }

    /// Stop current calls without preventing admission of later work.
    pub fn cancel_current(&self) {
        for token in self.inner.state().active.values() {
            token.lifetime.cancel();
        }
    }

    /// Stop admission and signal cancellation without waiting for cleanup.
    pub fn stop(&self) {
        let mut state = self.inner.state();
        state.stopped = true;
        for token in state.active.values() {
            token.lifetime.cancel();
        }
    }

    /// Stop admission, cancel outstanding calls, and wait for their cleanup.
    ///
    /// Upstream MCP connections stay open: they belong to the Host, and another
    /// service may be mid-Turn on them.
    /// Safe to call more than once.
    pub async fn shutdown(&self) {
        self.stop();
        loop {
            // Enabling the notification before checking is what makes this
            // race-free: a call finishing in between is still observed.
            let idle = self.inner.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self.inner.state().active.is_empty() {
                break;
            }
            idle.await;
        }
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
    // A skipped or hidden call shows nothing, so its formatter is a command
    // that would run for output nobody reads.
    let formats = matches!(tool.config.style().parameters, ParametersStyle::Custom(_))
        && tool.config.run() != RunMode::Skip
        && !tool.config.style().hidden
        && ask(inner, call, |reply| Interaction::RenderArguments { reply }).await?;
    // What the formatter's questions were answered with. The tool runs with the
    // same answers, so the call that executes is the one that was described.
    let mut answers = Answers::new();
    // `format = "ask"` holds a user-configured command back until the Host has
    // admitted the call.
    let mut formatted_arguments = None;
    if formats && tool.config.format() == FormatMode::Unattended {
        match describe(inner, call, &tool, &arguments, &mut answers, cancellation).await? {
            Ok(description) => formatted_arguments = Some(description),
            Err(result) => return record_without_executing(inner, call, arguments, result).await,
        }
    }
    let original_arguments = arguments.clone();
    // `run = "skip"` is the service's own decision, so it needs no Host
    // admission, but it resolves the call the same way a Host denial does.
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
    arguments = match admission {
        Admission::Run { arguments } => arguments,
        Admission::Skip { reason } => {
            return record_without_executing(inner, call, arguments, ToolResult::text(reason))
                .await;
        }
        Admission::Complete { result } => {
            return record_without_executing(inner, call, arguments, result).await;
        }
    };
    validate_arguments(&tool, &mut arguments)?;
    // Arguments the Host edited make any earlier formatting stale, so the
    // presentation is rebuilt from what will actually execute.
    if formats && (formatted_arguments.is_none() || arguments != original_arguments) {
        // Answers given about the original arguments may not hold for the
        // edited ones, so the formatter asks again.
        answers.clear();
        match describe(inner, call, &tool, &arguments, &mut answers, cancellation).await? {
            Ok(description) => formatted_arguments = Some(description),
            Err(result) => return record_without_executing(inner, call, arguments, result).await,
        }
    }
    let release = ask(inner, call, |reply| Interaction::Release {
        arguments: arguments.clone(),
        formatted_arguments,
        reply,
    })
    .await?;
    let (output, executed) = match release {
        ReleaseDecision::Execute => {
            let output = match attempt(
                inner,
                call,
                &tool,
                &arguments,
                Action::Run,
                &mut answers,
                cancellation,
            )
            .await?
            {
                Attempt::Completed(result) => CallOutput {
                    result,
                    delivery_decided: false,
                },
                Attempt::Settled(result) => CallOutput {
                    result,
                    delivery_decided: true,
                },
                Attempt::Failed(error) => return Err(error.into()),
            };
            (output, true)
        }
        ReleaseDecision::Complete { result } => (
            CallOutput {
                result,
                delivery_decided: true,
            },
            false,
        ),
    };
    deliver_result(inner, call, &tool, arguments, output, executed).await
}

/// Have the tool's argument formatter describe the call, asking the Host for
/// each answer it needs first.
///
/// Returns the description, or the result that settles the call instead: the
/// Host's, when it resolved one of the formatter's questions itself, or the
/// formatter's own failure.
/// A call whose formatter fails does not run, because nobody could see what it
/// would do.
async fn describe(
    inner: &Inner,
    call: &CallInfo,
    tool: &ConfiguredTool,
    arguments: &Map<String, Value>,
    answers: &mut Answers,
    cancellation: &CancellationToken,
) -> Result<Result<String, ToolResult>, ServiceError> {
    let failure = match attempt(
        inner,
        call,
        tool,
        arguments,
        Action::FormatArguments,
        answers,
        cancellation,
    )
    .await?
    {
        Attempt::Completed(result) if !result.is_error() => {
            return Ok(Ok(result.to_text().trim().to_owned()));
        }
        Attempt::Settled(result) => return Ok(Err(result)),
        Attempt::Completed(result) => formatter_failure(&result),
        Attempt::Failed(error) => error.to_string(),
    };
    Ok(Err(ToolResult::error(format!(
        "Tool '{}' was not executed because the argument formatter failed: {failure}",
        call.request.name
    ))))
}

/// The failure a formatter reported, as text for the sentence that settles the
/// call.
///
/// A transient failure's text is the `{"message", "trace"}` object a run
/// reports to the model; inside that sentence, only its message and trace are
/// shown, as plain text.
fn formatter_failure(result: &ToolResult) -> String {
    let text = result.to_text();
    if let Some(details) = result.error_details()
        && details.transient
        && let Ok(Value::Object(object)) = serde_json::from_str::<Value>(&text)
        && let Some(Value::String(message)) = object.get("message")
    {
        return CommandResult::format_error(message, &details.trace);
    }
    text
}

/// Record a call the Host resolved before it could execute.
async fn record_without_executing(
    inner: &Inner,
    call: &CallInfo,
    arguments: Map<String, Value>,
    result: ToolResult,
) -> Result<CallOutput, ServiceError> {
    ask(inner, call, |reply| Interaction::Record {
        recording: Box::new(Recording {
            arguments,
            // Nothing ran, so there is no unedited result behind the delivered
            // one.
            raw_result: None,
            result: result.clone(),
        }),
        reply,
    })
    .await?;
    Ok(CallOutput {
        result,
        delivery_decided: true,
    })
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
        result: raw_result,
        delivery_decided,
    } = output;
    let result = if delivery_decided {
        raw_result.clone()
    } else {
        match tool.config.result() {
            ResultMode::Skip => ToolResult::text("Result delivery skipped by configuration."),
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
    ask(inner, call, |reply| Interaction::Record {
        recording: Box::new(Recording {
            arguments,
            // A call the Host resolved at an earlier barrier never produced a
            // result of its own, so there is nothing unedited behind it.
            raw_result: (executed && !delivery_decided).then_some(raw_result),
            result: result.clone(),
        }),
        reply,
    })
    .await?;
    Ok(CallOutput {
        result,
        delivery_decided: true,
    })
}

/// How running the tool, or its argument formatter, ended.
enum Attempt {
    /// It ran to completion, successfully or reporting its own failure.
    Completed(ToolResult),

    /// The Host resolved the call with this result instead of answering one of
    /// its questions.
    Settled(ToolResult),

    /// It could not be run at all.
    Failed(ToolError),
}

/// Run the tool for `action` until it completes, asking the Host for each
/// answer it needs.
///
/// The tool and its argument formatter are the same call: both see the same
/// arguments, options, context, and answers, and differ only in `action`.
/// `answers` are the ones already given, which the first attempt sees, and
/// collect every answer given along the way.
async fn attempt(
    inner: &Inner,
    call: &CallInfo,
    tool: &ConfiguredTool,
    arguments: &Map<String, Value>,
    action: Action,
    answers: &mut Answers,
    cancellation: &CancellationToken,
) -> Result<Attempt, ServiceError> {
    let access = match &tool.access {
        Ok(access) => access.as_ref(),
        // A policy that never compiled ends a run with an error the caller
        // sees, and the call itself when it is only being described.
        Err(error) if action.is_run() => {
            return Ok(Attempt::Completed(ToolResult::error(error.to_string())));
        }
        Err(error) => return Err(error.clone().into()),
    };
    let progress = inner.progress.clone();
    let id = call.id;
    let stderr: StderrSink = Arc::new(move |line: &str| {
        drop(progress.send(Progress {
            id,
            line: line.into(),
        }));
    });
    // Built once: every attempt of this invocation runs the same tool, in the
    // same place, under the same policy. Only the answers grow.
    let execution = Execution {
        definition: &tool.definition,
        id: call.id.0.to_string(),
        arguments: Value::Object(arguments.clone()),
        action,
        config: &tool.config,
        root: &inner.root,
        access,
        invocation: &inner.invocation,
        builtins: &inner.builtins,
        runner: &inner.runner,
        upstream: &inner.upstream,
        cancellation: cancellation.clone(),
        stderr: Some(stderr),
    };
    loop {
        let outcome = match execute(&execution, answers).await {
            Ok(outcome) => outcome,
            Err(error) => return Ok(Attempt::Failed(error)),
        };
        match outcome {
            ExecutionOutcome::Cancelled { .. } => return Err(ServiceError::Cancelled),
            ExecutionOutcome::Completed { result, .. } => return Ok(Attempt::Completed(result)),
            ExecutionOutcome::NeedsInput { question, .. } => {
                if let Asked::Settled(result) =
                    ask_for_input(inner, call, question, answers).await?
                {
                    return Ok(Attempt::Settled(result));
                }
            }
        }
    }
}

/// How asking the Host for one answer ended.
enum Asked {
    /// The answer was validated and added to the call's answers.
    Answered,
    /// The Host resolved the call with this result instead of answering.
    Settled(ToolResult),
}

/// Ask the Host to answer `question`, and add the answer to `answers`.
///
/// An answer outside the question's answer shape fails the call.
async fn ask_for_input(
    inner: &Inner,
    call: &CallInfo,
    mut question: Question,
    answers: &mut Answers,
) -> Result<Asked, ServiceError> {
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
        InputAnswer::Complete { result } => return Ok(Asked::Settled(result)),
    };
    if !Node::root(&Value::Object(request.schema())).permits(&answer) {
        return Err(ServiceError::InvalidAnswer(request.id.clone()));
    }
    answers.insert(request.id.to_string(), answer);
    Ok(Asked::Answered)
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
