//! Per-call execution and private MCP Host interactions.
//!
//! [`Service::start_call`] is the execution entry point for the MCP handler.
//! Calls run independently of their result receivers.
//! Dropping a receiver does not cancel or retry work; use [`Call::cancel`] or
//! [`Service::cancel_current`].
//! The MCP Host must drain [`HostReceiver`] while calls are outstanding.
//!
//! A call to a tool with `fan_out` configured that carries an envelope runs as
//! a parent invocation with one child invocation per operation; see
//! [`fan_out`].
//! Each child talks to the Host like any call, tagged with its [`Operation`],
//! and ends with [`Interaction::Settled`]; the parent folds their results and
//! owns the one [`Interaction::Record`].
//!
//! [`fan_out`]: super::fan_out

use std::{
    collections::HashMap,
    error::Error as StdError,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use camino::Utf8PathBuf;
use indexmap::IndexMap;
use jp_config::conversation::tool::{
    FanOut, FormatMode, ResultMode, RunMode, ToolConfigWithDefaults, style::ParametersStyle,
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
    Answers, CommandPlugins, CommandResult, Execution, ExecutionOutcome, InvocationContext,
    StderrSink,
    builtin::BuiltinExecutors,
    execute,
    fan_out::{self, OperationOutcome, Split},
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
    ///
    /// For an operation of a fanned-out call this is the whole call, envelope
    /// included; the operation's own arguments arrive with `Prepare`.
    pub request: CallRequest,
    /// Which operation of a fanned-out call this invocation runs, or `None` for
    /// an ordinary call and for the fanned-out call itself.
    pub operation: Option<Operation>,
}

/// One operation of a fanned-out call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operation {
    /// The invocation of the call the operation belongs to.
    pub parent: InvocationId,
    /// Zero-based position of the operation in the caller's `ops` array.
    pub index: usize,
    /// How many operations the call carries.
    pub count: usize,
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
    /// Report how one operation of a fanned-out call ended.
    ///
    /// The last interaction of every operation, instead of `Record`: the
    /// operation is not recorded on its own, but folded into its call's
    /// `Record`.
    /// Not sent for an operation the Host resolved with `complete_call`, since
    /// the Host supplied that result itself.
    Settled {
        /// How the operation ended.
        settlement: Box<Recording>,
        /// Acknowledges that the Host has taken the settlement.
        reply: oneshot::Sender<HostReply<()>>,
    },
    /// Acknowledge final recording before returning the result to the caller.
    Record {
        /// What the Host is being asked to record.
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
    ///
    /// For a fanned-out call, the folded result of its operations.
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
            Self::Settled { reply, .. } | Self::Record { reply, .. } => reply.is_closed(),
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

    /// For an operation a `stop` policy ruled out, the one-based position of
    /// the operation whose failure stopped it.
    not_run: Option<usize>,
}

impl CallOutput {
    /// A result still subject to the tool's result-delivery policy.
    fn undecided(result: ToolResult) -> Self {
        Self {
            result,
            delivery_decided: false,
            not_run: None,
        }
    }

    /// A result the Host or the service has already settled on.
    fn decided(result: ToolResult) -> Self {
        Self {
            delivery_decided: true,
            ..Self::undecided(result)
        }
    }
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

    /// What each tool is advertised as, in the same order: its own definition,
    /// or the fan-out envelope around it.
    advertised: IndexMap<String, ToolDefinition>,
    upstream: Client,
    builtins: BuiltinExecutors,
    runner: Arc<dyn ProcessRunner>,
    command_plugins: CommandPlugins,
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

impl ActiveCall {
    /// Keep `id` in the active set until this guard drops.
    fn new(inner: &Arc<Inner>, id: InvocationId) -> Self {
        Self {
            inner: inner.clone(),
            id,
        }
    }
}

/// What a submitted call turns out to need.
enum Work {
    /// One invocation running the tool.
    Plain,
    /// One child invocation per operation, under the submitted call.
    FanOut {
        policy: FanOut,
        operations: Vec<Map<String, Value>>,
    },
    /// Nothing to run: the call is answered with this result as recorded.
    Refuse(ToolResult),
}

/// An invocation's entry in the active set, as its task needs it.
struct Control {
    id: InvocationId,
    lifetime: CancellationToken,
    attempt: CancellationToken,
    resume: Arc<Notify>,
}

/// Allocate an invocation and add it to the active set.
///
/// `lifetime` ends the invocation when cancelled; each attempt runs under a
/// child of it.
fn register(inner: &Arc<Inner>, lifetime: CancellationToken) -> Result<Control, ServiceError> {
    let mut state = inner.state();
    if state.stopped {
        return Err(ServiceError::Stopped);
    }
    state.next_id = state
        .next_id
        .checked_add(1)
        .ok_or(ServiceError::IdExhausted)?;
    let id = InvocationId(state.next_id);
    let attempt = lifetime.child_token();
    let resume = Arc::new(Notify::new());
    state.active.insert(id, CallControl {
        lifetime: lifetime.clone(),
        attempt: attempt.clone(),
        resume: resume.clone(),
        completion: None,
    });
    Ok(Control {
        id,
        lifetime,
        attempt,
        resume,
    })
}

/// Run one invocation of the tool to its result, across the attempts a Host
/// restart opens.
///
/// `gate` is the fan-out policy an operation runs under, with its position.
async fn drive(
    inner: &Arc<Inner>,
    call: &CallInfo,
    tool: &ConfiguredTool,
    arguments: &Map<String, Value>,
    gate: Option<(&Arc<Gate>, usize)>,
    control: Control,
) -> Result<CallOutput, ServiceError> {
    let _active = ActiveCall::new(inner, control.id);
    let Control {
        id,
        lifetime,
        mut attempt,
        resume,
    } = control;
    loop {
        let result = tokio::select! {
            biased;
            () = lifetime.cancelled() => Some(Err(ServiceError::Cancelled)),
            () = inner.host.closed() => Some(Err(ServiceError::HostDisconnected)),
            () = attempt.cancelled() => None,
            result = run_call(inner, call, tool.clone(), arguments.clone(), gate, &attempt) => {
                Some(result)
            }
        };
        if let Some(result) = result {
            return result;
        }
        // The MCP caller still owns the same pending request. Wait for the Host
        // to re-prepare it before opening another attempt.
        tokio::select! {
            biased;
            () = lifetime.cancelled() => return Err(ServiceError::Cancelled),
            () = inner.host.closed() => return Err(ServiceError::HostDisconnected),
            () = resume.notified() => {},
        }
        let completion = inner
            .state()
            .active
            .get_mut(&id)
            .and_then(|control| control.completion.take());
        if let Some(result) = completion {
            return Ok(CallOutput::decided(result));
        }
        attempt = lifetime.child_token();
        if let Some(control) = inner.state().active.get_mut(&id) {
            control.attempt = attempt.clone();
        }
    }
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
    /// `runner` runs every subprocess: `local` tools, command plugin binaries,
    /// and argument formatters.
    /// `command_plugins` names the plugins admitted for this turn; a call to
    /// any other plugin fails as unavailable.
    /// The returned receiver is the private Host interface.
    pub fn new(
        tools: Vec<ConfiguredTool>,
        upstream: Client,
        builtins: BuiltinExecutors,
        runner: Arc<dyn ProcessRunner>,
        command_plugins: CommandPlugins,
        root: Utf8PathBuf,
        invocation: InvocationContext,
    ) -> Result<(Self, HostReceiver), ServiceError> {
        let mut catalog = IndexMap::new();
        let mut advertised = IndexMap::new();
        for tool in tools {
            if tool.config.access().is_some() && matches!(tool.access, Ok(None)) {
                return Err(ServiceError::MissingAccessPolicy(tool.definition.name));
            }
            let name = tool.definition.name.clone();
            let definition = if tool.config.fan_out().is_some() {
                fan_out::advertise(&tool.definition)
            } else {
                tool.definition.clone()
            };
            if catalog.insert(name.clone(), tool).is_some() {
                return Err(ServiceError::DuplicateTool(name));
            }
            advertised.insert(name, definition);
        }
        let (host, receiver) = mpsc::channel(32);
        let (progress, _) = broadcast::channel(64);
        Ok((
            Self {
                inner: Arc::new(Inner {
                    tools: catalog,
                    advertised,
                    upstream,
                    builtins,
                    runner,
                    command_plugins,
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
    ///
    /// A tool with `fan_out` configured is advertised with the envelope schema
    /// around its own, which is what every caller, JP's own provider requests
    /// included, should be shown.
    pub fn definitions(&self) -> impl Iterator<Item = &ToolDefinition> {
        self.inner.advertised.values()
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
        let tool = inner
            .tools
            .get(&request.name)
            .cloned()
            .ok_or_else(|| ToolError::NotFound {
                name: request.name.clone(),
            })?;

        let work = match tool.config.fan_out() {
            None => Work::Plain,
            Some(policy) => match fan_out::split(&tool.definition, &request.arguments) {
                Split::Bare => Work::Plain,
                Split::Envelope(operations) => Work::FanOut { policy, operations },
                Split::Malformed(error) => {
                    Work::Refuse(ToolResult::error(error.message(&request.name)))
                }
            },
        };

        let cancellation = CancellationToken::new();
        let control = register(&inner, cancellation.clone())?;
        let id = control.id;
        let (sender, result) = oneshot::channel();
        tokio::spawn(async move {
            let arguments = request.arguments.clone();
            let call = CallInfo {
                id,
                request,
                operation: None,
            };
            let result = match work {
                Work::Plain => drive(&inner, &call, &tool, &arguments, None, control).await,
                Work::Refuse(result) => {
                    let _active = ActiveCall::new(&inner, id);
                    tokio::select! {
                        biased;
                        () = control.lifetime.cancelled() => Err(ServiceError::Cancelled),
                        () = inner.host.closed() => Err(ServiceError::HostDisconnected),
                        output = record_without_executing(&inner, &call, arguments, result) => output,
                    }
                }
                Work::FanOut { policy, operations } => {
                    let _active = ActiveCall::new(&inner, id);
                    tokio::select! {
                        biased;
                        () = control.lifetime.cancelled() => Err(ServiceError::Cancelled),
                        () = inner.host.closed() => Err(ServiceError::HostDisconnected),
                        output = run_fan_out(&inner, &call, &tool, policy, operations, &control.lifetime) => output,
                    }
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

/// Coerce, default, and validate `arguments` against the tool's own schema.
pub(super) fn validate_arguments(
    definition: &ToolDefinition,
    arguments: &mut Map<String, Value>,
) -> Result<(), ServiceError> {
    definition.coerce_arguments(arguments);
    apply_parameter_defaults(arguments, &definition.parameters);
    validate_tool_arguments(arguments, &definition.parameters)?;
    for (name, node) in Node::root(&definition.parameters).properties() {
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
    mut arguments: Map<String, Value>,
    gate: Option<(&Arc<Gate>, usize)>,
    cancellation: &CancellationToken,
) -> Result<CallOutput, ServiceError> {
    validate_arguments(&tool.definition, &mut arguments)?;
    // A skipped or hidden call shows nothing, so its formatter is a command
    // that would run for output nobody reads.
    let formats = has_formatter(&tool)
        && tool.config.run() != RunMode::Skip
        && !tool.config.style().hidden
        && ask(inner, call, |reply| Interaction::RenderArguments { reply }).await?;
    // What the formatter's questions were answered with. The tool runs with the
    // same answers, so the call that executes is the one that was described.
    let mut answers = Answers::new();
    // `format = "ask"` holds a formatter back until the Host has admitted the
    // call: it is a program, which should not run unprompted.
    let mut formatted_arguments = None;
    if formats && tool.config.format() == FormatMode::Allow {
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
    validate_arguments(&tool.definition, &mut arguments)?;
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
            // An operation of a fanned-out call waits here for its turn under
            // the call's concurrency limit, and is not run at all once an
            // earlier operation failed under `on_error = "stop"`.
            let permit = match gate {
                None => None,
                Some((gate, index)) => match gate.enter(index).await {
                    Entry::Run(permit) => Some(permit),
                    Entry::NotRun { after } => {
                        return settle_not_run(inner, call, arguments, after).await;
                    }
                },
            };
            let attempted = attempt(
                inner,
                call,
                &tool,
                &arguments,
                Action::Run,
                &mut answers,
                cancellation,
            )
            .await;
            // Recorded before the permit is released, whichever way the attempt
            // ended, so the next operation cannot start between this failure
            // and the gate learning of it. The tool's own result decides, not
            // the one delivered after result-mode policy.
            if let Some((gate, index)) = gate {
                let failed = match &attempted {
                    Ok(Attempt::Completed(result) | Attempt::Settled(result)) => result.is_error(),
                    Ok(Attempt::Failed(_)) | Err(_) => true,
                };
                if failed {
                    gate.fail(index);
                }
            }
            drop(permit);
            let output = match attempted? {
                Attempt::Completed(result) => CallOutput::undecided(result),
                Attempt::Settled(result) => CallOutput::decided(result),
                Attempt::Failed(error) => return Err(error.into()),
            };
            (output, true)
        }
        ReleaseDecision::Complete { result } => (CallOutput::decided(result), false),
    };
    deliver_result(inner, call, &tool, arguments, output, executed).await
}

/// Whether `tool` has an argument formatter to run: a configured command, or,
/// for `style.parameters = "tool"`, the tool itself.
///
/// A tool only describes itself when JP runs it as a subprocess, which is where
/// the action reaches it; the configuration refuses `tool` anywhere else.
fn has_formatter(tool: &ConfiguredTool) -> bool {
    match tool.config.style().parameters {
        ParametersStyle::Custom(_) => true,
        ParametersStyle::Tool => tool.config.source().is_subprocess(),
        _ => false,
    }
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

/// Conclude an invocation: `Record` for a call, `Settled` for an operation of a
/// fanned-out call.
async fn conclude(
    inner: &Inner,
    call: &CallInfo,
    recording: Recording,
) -> Result<(), ServiceError> {
    let recording = Box::new(recording);
    if call.operation.is_some() {
        ask(inner, call, |reply| Interaction::Settled {
            settlement: recording,
            reply,
        })
        .await
    } else {
        ask(inner, call, |reply| Interaction::Record {
            recording,
            reply,
        })
        .await
    }
}

/// Settle an operation a `stop` policy ruled out.
async fn settle_not_run(
    inner: &Inner,
    call: &CallInfo,
    arguments: Map<String, Value>,
    after: usize,
) -> Result<CallOutput, ServiceError> {
    let result = ToolResult::error(format!(
        "Operation not run: stopped after operation {after} failed."
    ));
    let mut output = record_without_executing(inner, call, arguments, result).await?;
    output.not_run = Some(after);
    Ok(output)
}

/// Run a fanned-out call: one child invocation per operation, folded into the
/// call's one `Record`.
async fn run_fan_out(
    inner: &Arc<Inner>,
    call: &CallInfo,
    tool: &ConfiguredTool,
    policy: FanOut,
    operations: Vec<Map<String, Value>>,
    lifetime: &CancellationToken,
) -> Result<CallOutput, ServiceError> {
    let count = operations.len();
    let gate = Arc::new(Gate::new(policy, count));

    // Whatever path this call leaves by, none of its operations keeps running.
    let children = lifetime.child_token();
    let _children = children.clone().drop_guard();

    let mut tasks = Vec::with_capacity(count);
    for (index, arguments) in operations.into_iter().enumerate() {
        let control = register(inner, children.child_token())?;
        let operation = CallInfo {
            id: control.id,
            request: call.request.clone(),
            operation: Some(Operation {
                parent: call.id,
                index,
                count,
            }),
        };
        let (inner, tool, gate) = (inner.clone(), tool.clone(), gate.clone());
        tasks.push(tokio::spawn(async move {
            let output = drive(
                &inner,
                &operation,
                &tool,
                &arguments,
                Some((&gate, index)),
                control,
            )
            .await;
            settle_operation(&inner, &operation, arguments, output, &gate, index).await
        }));
    }

    let mut outcomes = Vec::with_capacity(count);
    for task in tasks {
        outcomes.push(task.await.map_err(|_| ServiceError::TaskLost)??);
    }

    let result = fan_out::fold(&outcomes);
    conclude(inner, call, Recording {
        arguments: call.request.arguments.clone(),
        raw_result: None,
        result: result.clone(),
    })
    .await?;
    Ok(CallOutput::decided(result))
}

/// Turn how one operation's invocation ended into its folded outcome.
///
/// An operation that failed before concluding (invalid arguments, an answer the
/// tool cannot take) still has a Host waiting on it, so it is settled here with
/// the error.
/// One the Host cancelled or completed itself needs no settlement: the Host
/// already knows how it ended.
async fn settle_operation(
    inner: &Inner,
    call: &CallInfo,
    arguments: Map<String, Value>,
    output: Result<CallOutput, ServiceError>,
    gate: &Gate,
    index: usize,
) -> Result<OperationOutcome, ServiceError> {
    let output = match output {
        Ok(output) => output,
        // Nothing will record the call either.
        Err(error @ (ServiceError::HostDisconnected | ServiceError::Stopped)) => {
            gate.finish(index, false);
            return Err(error);
        }
        Err(ServiceError::Cancelled) => {
            CallOutput::decided(ToolResult::error("Operation cancelled."))
        }
        Err(error) => {
            let result = ToolResult::error(error.to_string());
            match record_without_executing(inner, call, arguments, result.clone()).await {
                Ok(output) => output,
                Err(error @ (ServiceError::HostDisconnected | ServiceError::Stopped)) => {
                    gate.finish(index, false);
                    return Err(error);
                }
                Err(_) => CallOutput::decided(result),
            }
        }
    };

    // An operation that ran told the gate about its own failure before giving
    // up its slot, from the tool's result rather than the delivered one, which
    // `result = "skip"` turns into a success. This catches one that ended
    // without running, whose delivered result is all there is.
    let failed = output.not_run.is_none() && output.result.is_error();
    gate.finish(index, failed);

    Ok(match output.not_run {
        Some(after) => OperationOutcome::NotRun { after },
        None if output.result.is_error() => OperationOutcome::Error(output.result.to_text()),
        None => OperationOutcome::Ok(output.result.to_text()),
    })
}

/// The concurrency and error policy the operations of one fanned-out call run
/// under.
///
/// An operation enters after the Host releases it and before it executes.
/// Under a concurrency limit operations start in the order the caller wrote
/// them: one waits until every earlier operation has started or settled.
struct Gate {
    limit: Option<usize>,
    stops_on_error: bool,
    state: Mutex<GateState>,
    changed: Notify,
}

struct GateState {
    /// Per operation, whether it has started or settled without starting.
    passed: Vec<bool>,
    /// Operations executing now.
    in_flight: usize,
    /// One-based position of the earliest operation that failed.
    first_failure: Option<usize>,
}

/// Whether an operation may execute.
enum Entry {
    /// Execute, holding a concurrency slot until the permit drops.
    Run(Permit),
    /// Do not execute: an earlier operation failed under `on_error = "stop"`.
    NotRun { after: usize },
}

/// A concurrency slot, released when dropped.
struct Permit(Arc<Gate>);

impl Drop for Permit {
    fn drop(&mut self) {
        self.0.state().in_flight -= 1;
        self.0.changed.notify_waiters();
    }
}

impl Gate {
    fn new(policy: FanOut, count: usize) -> Self {
        Self {
            limit: policy.concurrency,
            stops_on_error: policy.stops_on_error(),
            state: Mutex::new(GateState {
                passed: vec![false; count],
                in_flight: 0,
                first_failure: None,
            }),
            changed: Notify::new(),
        }
    }

    fn state(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait until operation `index` may execute, or learn that it may not.
    async fn enter(self: &Arc<Self>, index: usize) -> Entry {
        loop {
            // Enabled before the state is read, so a change in between still
            // wakes this waiter.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();

            if let Some(entry) = self.try_enter(index) {
                self.changed.notify_waiters();
                return entry;
            }
            changed.await;
        }
    }

    /// Decide operation `index`'s entry now, or `None` if it has to wait.
    fn try_enter(self: &Arc<Self>, index: usize) -> Option<Entry> {
        let mut state = self.state();
        if self.stops_on_error
            && let Some(after) = state.first_failure
        {
            state.passed[index] = true;
            return Some(Entry::NotRun { after });
        }
        let turn = self.limit.is_none_or(|limit| {
            state.in_flight < limit && state.passed[..index].iter().all(|passed| *passed)
        });
        if !turn {
            return None;
        }
        state.in_flight += 1;
        state.passed[index] = true;
        Some(Entry::Run(Permit(self.clone())))
    }

    /// Record that operation `index` failed.
    fn fail(&self, index: usize) {
        record_failure(&mut self.state(), index);
        self.changed.notify_waiters();
    }

    /// Record that operation `index` settled, and whether it failed.
    ///
    /// Both are recorded under one lock, so an operation entering in between
    /// never sees this one as passed but not failed.
    fn finish(&self, index: usize, failed: bool) {
        let mut state = self.state();
        state.passed[index] = true;
        if failed {
            record_failure(&mut state, index);
        }
        drop(state);
        self.changed.notify_waiters();
    }
}

/// Keep the earliest failed operation's one-based position.
fn record_failure(state: &mut GateState, index: usize) {
    let position = index + 1;
    if state.first_failure.is_none_or(|first| position < first) {
        state.first_failure = Some(position);
    }
}

/// Record a call the Host resolved before it could execute.
async fn record_without_executing(
    inner: &Inner,
    call: &CallInfo,
    arguments: Map<String, Value>,
    result: ToolResult,
) -> Result<CallOutput, ServiceError> {
    conclude(inner, call, Recording {
        arguments,
        // Nothing ran, so there is no unedited result behind the delivered one.
        raw_result: None,
        result: result.clone(),
    })
    .await?;
    Ok(CallOutput::decided(result))
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
        ..
    } = output;
    let result = if delivery_decided {
        raw_result.clone()
    } else {
        match tool.config.result() {
            ResultMode::Skip => ToolResult::text("Result delivery skipped by configuration."),
            ResultMode::Allow => raw_result.clone(),
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
    conclude(inner, call, Recording {
        arguments,
        // A call the Host resolved at an earlier barrier never produced a
        // result of its own, so there is nothing unedited behind it.
        raw_result: (executed && !delivery_decided).then_some(raw_result),
        result: result.clone(),
    })
    .await?;
    Ok(CallOutput::decided(result))
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
        command_plugins: &inner.command_plugins,
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
