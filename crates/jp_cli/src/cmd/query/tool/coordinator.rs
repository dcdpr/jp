//! Tool call coordination for the query stream pipeline.
//!
//! The [`ToolCoordinator`] takes each tool call from the moment the assistant
//! finishes requesting it to the response the conversation records.
//! [`submit`] hands it a call, [`handle`] advances its calls on each event
//! [`next_event`] yields while the response is still streaming, and [`finish`]
//! drives them to their responses once it has ended.
//!
//! # Calls in flight
//!
//! A call runs as far as it can on its own as soon as it arrives: its argument
//! formatter describes it, and a question the formatter or the tool asks is
//! routed straight away.
//! The assistant answers the questions routed to it for several calls at once.
//!
//! # The terminal
//!
//! One prompt is on screen at a time, and calls are announced in the order the
//! assistant sent them.
//! A call's header, description, approval prompt, and questions to the user
//! wait until every earlier call has been announced or settled, so a call is
//! never shown ahead of one that came before it.
//! A call skipped by a "no" remembered for the turn is settled when its turn
//! comes, and nothing of it is shown.
//!
//! # Questions
//!
//! Every question takes one route, whichever step asked it: the tool's or its
//! argument formatter's, before approval or while the tool runs.
//! An answer remembered for the turn or configured for the tool comes first,
//! then the user or the assistant, as the question's `target` says, and each
//! round-trip is recorded as an inquiry.
//! A question still open when its call is settled is withdrawn.
//!
//! # Release
//!
//! Nothing runs until the response has finished streaming and every call has
//! been approved or settled.
//! The approved calls then run in parallel, and their results are reviewed and
//! recorded.
//!
//! [`finish`]: ToolCoordinator::finish
//! [`handle`]: ToolCoordinator::handle
//! [`next_event`]: ToolCoordinator::next_event
//! [`submit`]: ToolCoordinator::submit

use std::{
    collections::{HashMap, VecDeque},
    future::pending,
    sync::Arc,
};

use indexmap::IndexMap;
use inquire::error::InquireError;
use jp_config::{
    conversation::tool::{
        QuestionTarget, ResultMode, RunMode, ToolsConfig, style::ParametersStyle,
    },
    interrupt::ToolInterruptConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{
        CancellationReason, InquiryAnswerType, InquiryId, InquiryQuestion, InquiryRequest,
        InquiryResponse, InquirySource, SelectOption, ToolCallRequest, ToolCallResponse,
    },
};
use jp_llm::query::ToolExecution;
use jp_mcp::server::StderrSink;
use jp_printer::Printer;
use jp_tool::{AnswerType, PersistLevel, Question};
use jp_workspace::ConversationMut;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use url::Url;

use super::{
    ToolRenderer,
    executor::{Executor, ExecutorError, ExecutorResult, ExecutorSource, PermissionInfo, Review},
    inquiry::{self, InquiryBackend, InquiryError},
    prompter::{PermissionResult, ToolPrompter},
};
use crate::{
    Error,
    cmd::query::{
        interrupt::{
            TurnInterrupts,
            signals::{
                InterruptUi, ToolInterruptResult, apply_tool_interrupt, as_tool_interrupt,
                handle_tool_interrupt,
            },
        },
        turn::state::TurnState,
    },
    signals::SignalRouter,
};

/// What the rest of the turn lends the coordinator while it handles an event.
pub(crate) struct Host<'a> {
    /// Runs the prompts a call needs: approval, questions, result review.
    pub prompter: &'a Arc<ToolPrompter>,

    /// Answers a question routed to an assistant instead of the user.
    pub inquiry_backend: &'a Arc<dyn InquiryBackend>,

    /// The conversation each inquiry is recorded on.
    pub conv: &'a ConversationMut,

    /// What the user asked to remember for the turn, and the inquiry counter.
    pub turn_state: &'a mut TurnState,

    /// Draws each call's header, description, and result.
    pub renderer: &'a mut ToolRenderer,

    /// Shades the prompts a call opens with the call's reasoning region.
    pub printer: &'a Printer,

    /// Whether a user is there to answer a prompt at all.
    pub interactive: bool,
}

/// Something a call's work in flight reported back.
#[derive(Debug)]
pub(crate) enum ToolEvent {
    /// An executor step finished.
    Step { call: usize, result: ExecutorResult },

    /// The call's approval prompt closed.
    Permission {
        call: usize,
        result: Result<PermissionResult, String>,
    },

    /// The user answered a question.
    Answered {
        call: usize,
        inquiry_id: InquiryId,
        question_id: String,
        answer: Value,
        persist_level: PersistLevel,
        /// Whether the persisted response must be recorded as `Redacted` (the
        /// question's answer type is `Secret`).
        redact: bool,
    },

    /// A question prompt closed without an answer.
    Unanswered {
        call: usize,
        inquiry_id: InquiryId,
        /// Why the prompt closed without an answer, mapped from the prompter
        /// error at the prompt site.
        reason: CancellationReason,
    },

    /// The assistant answered a question, or could not.
    Inquired {
        call: usize,
        inquiry_id: InquiryId,
        question_id: String,
        question_text: String,
        result: Result<Value, InquiryError>,
    },

    /// The call's result review closed.
    Reviewed { call: usize, review: Review },
}

#[derive(Debug, Default)]
pub struct ExecutionResult {
    /// What the Host settled on per tool call, keyed by the call's id.
    ///
    /// Merging these back into the stream's order is the caller's job.
    pub reviews: IndexMap<String, Review>,

    /// How the execution phase ended, and what the caller should do next.
    pub outcome: ExecutionOutcome,
}

/// How a tool execution phase ended.
///
/// Variants are ordered by severity: interrupts can arrive on every event-loop
/// iteration while cancelled tools drain, and a later, less severe choice must
/// not downgrade an earlier one (see [`ExecutionOutcome::upgrade`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ExecutionOutcome {
    /// Every tool produced a response (including cancellation responses filled
    /// in for tools cancelled via "Stop & respond"); the caller should commit
    /// the responses and continue the turn.
    #[default]
    Completed,

    /// The user chose "Restart" (or configured `interrupt.tool_call.action =
    /// "restart"`).
    /// The running tools have been cancelled; the caller should re-execute the
    /// batch.
    Restart,

    /// The user chose "Stop (cancel & exit)" (or configured
    /// `interrupt.tool_call.action = "stop"`).
    /// The running tools have been cancelled and their cancellation responses
    /// filled in; the caller should record the responses and end the turn
    /// without a follow-up request.
    Stopped,

    /// The user escalated past the tool interrupt menu (cancelled it with
    /// Ctrl-C).
    /// The running tools have been cancelled; the caller should begin a
    /// graceful shutdown.
    Escalated,
}

impl ExecutionOutcome {
    /// Record a newly observed outcome, keeping the most severe one.
    ///
    /// Without this, a second interrupt during the cancellation drain could
    /// downgrade the outcome, e.g. a "Stop & respond" reply clearing an earlier
    /// "Stop (cancel & exit)".
    fn upgrade(&mut self, next: Self) {
        *self = (*self).max(next);
    }
}

/// Route a tool's stderr into the progress window, when one is showing and this
/// tool belongs in it.
///
/// Two levels gate the window and both must allow it: `style.tool_call.progress
/// .stderr_rows` decides whether a window exists and how tall it is, and
/// `conversation.tools.<name>.style.print_stderr` decides whether this tool
/// feeds it.
/// Rows are screen space, shared by every tool running at once, so only
/// membership can be answered per-tool.
///
/// The closure runs on the forwarder's read loop, so it must not block:
/// `LineSink::push` writes into a bounded shared buffer and returns, which is
/// what keeps the child off a full pipe while the terminal catches up.
fn stderr_sink(
    renderer: &ToolRenderer,
    tools_config: &ToolsConfig,
    tool_name: &str,
) -> Option<StderrSink> {
    if !tools_config.get(tool_name)?.style().print_stderr {
        return None;
    }

    let sink = renderer.progress_source(tool_name)?;

    Some(Arc::new(move |line: &str| sink.push(line)))
}

/// The tool calls of one response, from arrival to their recorded responses.
struct Batch {
    /// Every call the batch took, in the order the assistant sent them.
    calls: Vec<Call>,

    /// Calls settled on arrival, because their tool is not available.
    unavailable: Vec<ToolCallResponse>,

    /// Where each call's work in flight reports back.
    events: mpsc::UnboundedSender<ToolEvent>,
    receiver: mpsc::UnboundedReceiver<ToolEvent>,

    /// Parent of every token handed to this batch's work in flight.
    cancellation: CancellationToken,

    /// Prompts waiting for the terminal.
    prompts: VecDeque<Prompt>,

    /// Whether a prompt owns the terminal.
    prompting: bool,

    /// Whether the response has finished streaming, so no more calls arrive.
    stream_ended: bool,

    /// Whether the approved calls were released to run.
    released: bool,

    /// Whether the progress row was claimed for the released calls.
    progress: bool,
}

impl Batch {
    fn new() -> Self {
        let (events, receiver) = mpsc::unbounded_channel();
        Self {
            calls: Vec::new(),
            unavailable: Vec::new(),
            events,
            receiver,
            cancellation: CancellationToken::new(),
            prompts: VecDeque::new(),
            prompting: false,
            stream_ended: false,
            released: false,
            progress: false,
        }
    }

    /// The first call not yet announced: the only one whose approval-stage
    /// prompts may use the terminal.
    fn cursor(&self) -> usize {
        self.calls
            .iter()
            .position(|call| !call.announced)
            .unwrap_or(self.calls.len())
    }

    /// Whether every call has a response.
    fn settled(&self) -> bool {
        self.calls.iter().all(|call| call.review.is_some())
    }
}

/// One tool call in a batch.
struct Call {
    tool_id: String,
    tool_name: String,
    executor: Arc<dyn Executor>,

    /// Answers to the call's questions, which its tool and formatter both see.
    answers: IndexMap<String, Value>,

    /// Where the tool's stderr goes while it runs, set when it is released.
    stderr: Option<StderrSink>,

    /// What the call is waiting for.
    wait: Wait,

    /// Whether the call has been approved, or settled without running.
    decided: bool,

    /// Whether the call has been shown, or settled with nothing to show.
    announced: bool,

    /// Whether the call was released to run.
    released: bool,

    /// What was drawn for the call ahead of its approval prompt, and the
    /// arguments it was drawn from.
    pre_render: Option<PreRendered>,

    /// The question the call is waiting on an answer to.
    question: Option<OpenQuestion>,

    /// What the Host settled on.
    review: Option<Review>,
}

/// What a call is waiting for.
#[derive(Debug)]
enum Wait {
    /// An executor step is running.
    Step,

    /// A question is out, with the user or the assistant.
    Answer,

    /// Parked at admission, for its turn at the terminal.
    Admission,

    /// Its approval prompt is open.
    Approval(PermissionInfo),

    /// Parked at release, for the batch to be released.
    Release,

    /// Its result is waiting to be reviewed.
    Review,

    /// Settled.
    Done,
}

/// What a call showed ahead of its approval prompt.
struct PreRendered {
    /// Content to persist for replay, when the style produced any.
    content: Option<String>,

    /// The arguments it was drawn from; an edit at the prompt makes it stale.
    arguments: Map<String, Value>,
}

/// A question a call is waiting on.
struct OpenQuestion {
    inquiry_id: InquiryId,

    /// Cancels the assistant's answer; `None` for a question the user answers.
    inquiry: Option<CancellationToken>,
}

/// Something waiting for the terminal.
enum Prompt {
    /// A question for the user.
    Question {
        call: usize,
        question: Question,
        inquiry_id: InquiryId,
    },

    /// A result waiting to be reviewed.
    Review {
        call: usize,
        response: ToolCallResponse,
        mode: ResultMode,
    },
}

impl Prompt {
    fn call(&self) -> usize {
        match self {
            Self::Question { call, .. } | Self::Review { call, .. } => *call,
        }
    }
}

/// The calls a "stop" from the interrupt menu cancelled, and what they answer
/// with.
#[derive(Default)]
struct Stop {
    /// The calls that had no response when the user stopped them.
    cancelled: Vec<usize>,

    /// The user's message, which every cancelled call answers with; `None`
    /// answers each with its tool's configured cancellation response.
    message: Option<String>,
}

/// An executor step to run.
enum Step {
    /// Submit the call.
    Prepare { render_arguments: bool },
    /// Admit the call.
    Approve,
    /// Release the call, or answer the question it is waiting on.
    Execute,
}

/// What rendering a tool call before its approval prompt produced.
#[derive(Debug)]
enum PreRender {
    /// Rendered; the content shown, when the style produced any.
    Ready(Option<String>),

    /// Held back until the call is admitted, because its formatter is a
    /// user-configured command the execution service has not run yet.
    Deferred,
}

/// Result of [`ToolCoordinator::decide_permission`] for a single tool.
#[derive(Debug)]
pub enum PermissionDecision {
    /// Tool can run immediately (unattended, persisted approval, non-TTY).
    Approved,
    /// Tool should not run (persisted skip).
    Skipped(ToolCallResponse),
    /// Requires an interactive user prompt before deciding.
    NeedsPrompt(PermissionInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallState {
    ReceivingArguments { name: String },
    Queued,
    AwaitingPermission,
    Running,
    AwaitingInput,
    AwaitingResultEdit,
    Completed,
}

impl ToolCallState {
    #[must_use]
    pub fn is_prompting(&self) -> bool {
        matches!(
            self,
            Self::AwaitingPermission | Self::AwaitingInput | Self::AwaitingResultEdit
        )
    }
}

/// Converts a `jp_tool::Question` into an `InquiryQuestion` for recording in
/// the conversation stream.
fn tool_question_to_inquiry_question(q: &Question) -> InquiryQuestion {
    let answer_type = match &q.answer_type {
        AnswerType::Boolean => InquiryAnswerType::Boolean,
        AnswerType::Select { options } => InquiryAnswerType::Select {
            options: options
                .iter()
                .map(|o| SelectOption::from(o.as_str()))
                .collect(),
        },
        AnswerType::Text => InquiryAnswerType::Text,
        AnswerType::Secret => InquiryAnswerType::Secret,
    };

    let mut iq = InquiryQuestion::new(q.text.clone(), answer_type);
    if let Some(default) = &q.default {
        iq = iq.with_default(default.clone());
    }

    iq
}

pub struct ToolCoordinator {
    /// The calls of the response being handled, once one has arrived.
    batch: Option<Batch>,
    tool_states: HashMap<String, ToolCallState>,
    tools_config: ToolsConfig,
    interrupt_config: ToolInterruptConfig,
    executor_source: Box<dyn ExecutorSource>,
    /// Rendered custom argument output for approved calls.
    /// Keyed by tool call ID.
    /// Drained by the turn loop to write into event metadata.
    rendered_arguments: HashMap<String, String>,
}

impl ToolCoordinator {
    /// The endpoint used by provider-owned tool dispatch.
    pub fn endpoint(&self) -> Option<Url> {
        self.executor_source.endpoint()
    }

    /// Bind upcoming tool observations to the selected dispatch contract.
    pub fn set_execution(&self, execution: ToolExecution) -> Result<(), ExecutorError> {
        self.executor_source.set_execution(execution)
    }

    pub fn new(tools_config: ToolsConfig, executor_source: Box<dyn ExecutorSource>) -> Self {
        Self {
            batch: None,
            tool_states: HashMap::new(),
            tools_config,
            interrupt_config: ToolInterruptConfig::default(),
            executor_source,
            rendered_arguments: HashMap::new(),
        }
    }

    /// Set the Ctrl-C behavior while tools are executing.
    ///
    /// Defaults to showing the interrupt menu.
    #[must_use]
    pub fn with_interrupt(mut self, config: ToolInterruptConfig) -> Self {
        self.interrupt_config = config;
        self
    }

    /// Drain accumulated rendered argument content.
    ///
    /// Returns `(tool_call_id, rendered_content)` pairs for the calls that were
    /// approved.
    /// The caller writes these into event metadata.
    pub fn drain_rendered_arguments(&mut self) -> HashMap<String, String> {
        std::mem::take(&mut self.rendered_arguments)
    }

    pub fn is_prompting(&self) -> bool {
        self.tool_states.values().any(ToolCallState::is_prompting)
    }

    /// Whether a call's prompt owns the terminal.
    pub(crate) fn prompt_active(&self) -> bool {
        self.batch.as_ref().is_some_and(|batch| batch.prompting)
    }

    pub(crate) fn set_tool_state(&mut self, tool_id: impl Into<String>, state: ToolCallState) {
        self.tool_states.insert(tool_id.into(), state);
    }

    /// Remove an abandoned argument preview without changing executable calls.
    pub(crate) fn discard_pending_tool(&mut self, tool_id: &str) {
        if matches!(
            self.tool_states.get(tool_id),
            Some(ToolCallState::ReceivingArguments { .. })
        ) {
            self.tool_states.remove(tool_id);
        }
    }

    pub fn parameter_style(&self, tool_name: &str) -> ParametersStyle {
        self.tools_config
            .get(tool_name)
            .map(|c| c.style().parameters.clone())
            .unwrap_or_default()
    }

    /// Pre-render a tool call ahead of its approval prompt.
    ///
    /// Built-in parameter styles ([`ParametersStyle::Json`],
    /// [`ParametersStyle::FunctionCall`], [`ParametersStyle::Off`]) always
    /// pre-render: they are pure transformations of the arguments map and the
    /// user needs to see the rendered call to make an informed approval
    /// decision.
    ///
    /// [`ParametersStyle::Custom`] renders whatever the execution service
    /// produced.
    /// A formatter configured with `format = "ask"` has not run yet at this
    /// point, which is [`PreRender::Deferred`].
    fn pre_render_for_prompt(&self, executor: &dyn Executor, renderer: &ToolRenderer) -> PreRender {
        let name = executor.tool_name();
        if matches!(self.parameter_style(name), ParametersStyle::Custom(_))
            && executor.formatted_arguments().is_none()
        {
            // Running a user-configured shell command before the user okays
            // the tool would be surprising, so `format = "ask"` holds the
            // formatter back until admission. Built-in styles are pure and
            // have no side effects, so they always render before the prompt.
            return PreRender::Deferred;
        }

        PreRender::Ready(self.render_executor(executor, renderer))
    }

    /// Render one tool call's arguments for display, returning the content to
    /// persist for replay.
    ///
    /// A `Custom` parameter style shows what the execution service's formatter
    /// produced.
    /// The formatter is a user-configured command, so it runs once, there,
    /// under the call's access policy and cancellation token, and never a
    /// second time here.
    fn render_executor(&self, executor: &dyn Executor, renderer: &ToolRenderer) -> Option<String> {
        let name = executor.tool_name();
        if self.is_hidden(name) {
            return None;
        }
        let ParametersStyle::Custom(_) = self.parameter_style(name) else {
            self.render_approved_tool(name, &executor.arguments(), renderer);
            return None;
        };
        // The service formats a call's arguments before releasing it, unless
        // the call is hidden or configured not to run. Both of those are
        // already handled, so no output here means there is no call to
        // announce: a bare header would say otherwise.
        let formatted = executor.formatted_arguments()?;
        renderer.render_custom_result(name, formatted)
    }

    /// Acknowledge the execution service after the conversation owner flushes.
    ///
    /// Until this runs, each call is still parked on its final barrier and its
    /// MCP response has not been returned to the caller.
    /// Every call is acknowledged even when one fails, so one call's
    /// disagreement does not strand the rest; the first failure is returned.
    pub async fn acknowledge_reviews(&self, reviews: Vec<Review>) -> Result<(), ExecutorError> {
        let mut failure = None;
        for review in reviews {
            if let Err(error) = self.executor_source.acknowledge(review).await {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }

    pub fn question_target(&self, tool_name: &str, question_id: &str) -> Option<QuestionTarget> {
        self.tools_config
            .get(tool_name)
            .and_then(|config| config.question_target(question_id).cloned())
    }

    pub fn static_answer(&self, tool_name: &str, question_id: &str) -> Option<serde_json::Value> {
        self.tools_config.get(tool_name).and_then(|config| {
            config
                .questions()
                .get(question_id)
                .and_then(|q| q.answer.clone())
        })
    }

    pub fn is_hidden(&self, tool_name: &str) -> bool {
        self.tools_config
            .get(tool_name)
            .is_some_and(|cfg| cfg.style().hidden)
    }

    /// Whether this tool's chrome joins the reasoning region it was called
    /// from, per `conversation.tools.<name>.style.joins_reasoning`.
    ///
    /// A tool with no config entry takes the value from the `*` defaults block,
    /// matching replay.
    pub fn joins_reasoning(&self, tool_name: &str) -> bool {
        self.tools_config
            .get(tool_name)
            .map_or(self.tools_config.defaults.style.joins_reasoning, |cfg| {
                cfg.style().joins_reasoning
            })
    }

    /// Return the response recorded for a cancelled call to `tool_name`: the
    /// tool's configured `cancellation_response`, falling back to the global
    /// default for unconfigured tools.
    pub fn cancellation_response(&self, tool_name: &str) -> String {
        self.tools_config.get(tool_name).map_or_else(
            || self.tools_config.defaults.cancellation_response.clone(),
            |config| config.cancellation_response().to_owned(),
        )
    }

    pub fn result_mode(&self, tool_name: &str) -> ResultMode {
        self.tools_config
            .get(tool_name)
            .map(|config| config.result())
            .unwrap_or_default()
    }

    /// Cancel the work in flight for the calls being handled.
    #[allow(dead_code)]
    pub fn cancel(&self) {
        if let Some(batch) = &self.batch {
            batch.cancellation.cancel();
        }
    }

    /// Drop the calls being handled, stopping their work in flight.
    ///
    /// Their requests get their responses elsewhere, as when a response that
    /// failed mid-stream is requested again.
    pub(crate) fn abandon(&mut self) {
        if let Some(batch) = self.batch.take() {
            batch.cancellation.cancel();
        }
    }

    /// Prepares a single executor for a tool call request.
    ///
    /// Returns the executor on success, or an error response if the tool cannot
    /// be resolved (e.g. missing from config or definitions).
    pub fn prepare_one(
        &mut self,
        request: ToolCallRequest,
    ) -> Result<Box<dyn Executor>, ToolCallResponse> {
        self.tool_states
            .insert(request.id.clone(), ToolCallState::Queued);

        if let Some(executor) = self
            .tools_config
            .get(&request.name)
            .and_then(|config| self.executor_source.create(request.clone(), config))
        {
            return Ok(executor);
        }

        warn!(tool = %request.name, "Tool not available, returning error to LLM");
        self.set_tool_state(&request.id, ToolCallState::Completed);
        Err(ToolCallResponse {
            id: request.id,
            result: Err(format!(
                "Tool '{}' is not available. It may have been available earlier in this \
                 conversation but is no longer enabled. Do not retry this tool until it it is \
                 available again in the list of enabled tools.",
                request.name,
            )),
        })
    }

    /// Renders the tool call header and arguments.
    ///
    /// Prints the header with inline-formatted arguments.
    /// A hidden tool renders nothing.
    ///
    /// A `Custom` parameter style is rendered by [`render_executor`] from the
    /// execution service's formatter output, not here.
    ///
    /// [`render_executor`]: Self::render_executor
    pub(crate) fn render_approved_tool(
        &self,
        tool_name: &str,
        arguments: &Map<String, Value>,
        tool_renderer: &ToolRenderer,
    ) {
        if self.is_hidden(tool_name) {
            return;
        }

        let style = self.parameter_style(tool_name);
        tool_renderer.render_approved(tool_name, arguments, &style);
    }

    /// Determines permission for a single tool without blocking on user input.
    ///
    /// Does NOT render any output.
    ///
    /// Returns one of:
    ///
    /// - `Approved`: tool can run immediately (unattended, persisted "y",
    ///   non-interactive)
    /// - `Skipped`: tool should not run (persisted "n")
    /// - `NeedsPrompt`: requires an interactive user prompt
    pub fn decide_permission(
        &mut self,
        executor: &dyn Executor,
        interactive: bool,
        turn_state: &TurnState,
    ) -> PermissionDecision {
        let Some(info) = executor.permission_info() else {
            return PermissionDecision::Approved;
        };

        if !interactive && matches!(info.run_mode, RunMode::Ask | RunMode::Edit) {
            self.set_tool_state(&info.tool_id, ToolCallState::Running);
            return PermissionDecision::Approved;
        }

        // Check for a persisted permission decision from earlier in this turn.
        match turn_state.remembered_permission(&info.tool_name) {
            Some(true) => {
                self.set_tool_state(&info.tool_id, ToolCallState::Running);
                PermissionDecision::Approved
            }
            Some(false) => {
                self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                PermissionDecision::Skipped(Self::remembered_skip(&info.tool_id))
            }
            None => PermissionDecision::NeedsPrompt(info),
        }
    }

    /// The response a call ends with when the user said "no" to its tool for
    /// the rest of the turn.
    fn remembered_skip(tool_id: &str) -> ToolCallResponse {
        ToolCallResponse {
            id: tool_id.to_owned(),
            result: Ok("Tool skipped by user (remembered).".to_string()),
        }
    }

    /// Applies the result of an interactive permission prompt.
    ///
    /// Call this after the user answers a prompt for a tool whose decision was
    /// [`PermissionDecision::NeedsPrompt`].
    /// An approval hands the arguments the user approved to `executor`; any
    /// other answer returns the response the call ends with instead.
    pub fn apply_permission_result(
        &mut self,
        result: Result<PermissionResult, String>,
        info: &PermissionInfo,
        turn_state: &mut TurnState,
        executor: &dyn Executor,
    ) -> Result<(), ToolCallResponse> {
        match result {
            Ok(PermissionResult::Run { arguments, persist }) => {
                if persist {
                    turn_state.remember_permission(&info.tool_name, true);
                }
                executor.set_arguments(arguments);
                self.set_tool_state(&info.tool_id, ToolCallState::Running);
                Ok(())
            }
            Ok(PermissionResult::Skip { reason, persist }) => {
                if persist {
                    turn_state.remember_permission(&info.tool_name, false);
                }
                self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                let msg = if let Some(r) = reason {
                    format!("Tool skipped by user: {r}")
                } else {
                    "Tool skipped by user.".to_string()
                };
                Err(ToolCallResponse {
                    id: info.tool_id.clone(),
                    result: Ok(msg),
                })
            }
            Err(error) => {
                self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                Err(ToolCallResponse {
                    id: info.tool_id.clone(),
                    result: Err(format!("Permission prompt failed: {error}")),
                })
            }
        }
    }

    /// Take a tool call the assistant finished requesting, and start it.
    ///
    /// Its formatter describes it and it asks what it needs to right away; what
    /// needs the terminal waits for the calls before it.
    pub(crate) fn submit(&mut self, request: ToolCallRequest, host: &mut Host<'_>) {
        if self.batch.is_none() {
            // A new response: the states left by the last one describe calls
            // that are gone.
            self.tool_states.clear();
            self.batch = Some(Batch::new());
        }
        let executor = self.prepare_one(request);
        let Some(mut batch) = self.batch.take() else {
            return;
        };
        match executor {
            Err(response) => batch.unavailable.push(response),
            Ok(executor) => {
                let executor: Arc<dyn Executor> = Arc::from(executor);
                // Formatting a call the user already said no to would run a
                // formatter command for output nobody sees.
                let remembered_denial = host.interactive
                    && executor.needs_permission()
                    && host.turn_state.remembered_permission(executor.tool_name()) == Some(false);
                let render_arguments = !self.is_hidden(executor.tool_name()) && !remembered_denial;
                let index = batch.calls.len();
                batch.calls.push(Call {
                    tool_id: executor.tool_id().to_owned(),
                    tool_name: executor.tool_name().to_owned(),
                    executor,
                    answers: IndexMap::new(),
                    stderr: None,
                    wait: Wait::Step,
                    decided: false,
                    announced: false,
                    released: false,
                    pre_render: None,
                    question: None,
                    review: None,
                });
                Self::spawn_step(&mut batch, index, Step::Prepare { render_arguments });
            }
        }
        self.pump(&mut batch, host);
        self.batch = Some(batch);
    }

    /// Wait for the next thing a call's work in flight reports.
    ///
    /// Never resolves while there are no calls.
    pub(crate) async fn next_event(&mut self) -> ToolEvent {
        match &mut self.batch {
            // The batch holds a sender itself, so the channel never closes.
            Some(batch) => match batch.receiver.recv().await {
                Some(event) => event,
                None => pending().await,
            },
            None => pending().await,
        }
    }

    /// Advance the calls with `event`, and hand the terminal on.
    pub(crate) fn handle(&mut self, event: ToolEvent, host: &mut Host<'_>) {
        let Some(mut batch) = self.batch.take() else {
            return;
        };
        self.dispatch(&mut batch, event, host);
        self.pump(&mut batch, host);
        self.batch = Some(batch);
    }

    /// Drive the calls to their responses, now that the response has finished
    /// streaming.
    ///
    /// Nothing runs until every call has been approved or settled; the approved
    /// calls then run in parallel.
    /// Returns what the Host settled on for each call, including calls handed
    /// over by [`submit`] while the response streamed.
    ///
    /// `interactive` gates every question and result prompt.
    /// The elapsed-time progress row takes no parameter: it is a status region,
    /// so the printer's own terminal capability decides whether it renders.
    ///
    /// `interrupts` carries interrupts from a client driving the turn from
    /// outside the process; one is taken per call, and the rest stay queued.
    ///
    /// [`submit`]: Self::submit
    pub(crate) async fn finish(
        &mut self,
        host: &mut Host<'_>,
        signals: &SignalRouter,
        interrupt_ui: &mut InterruptUi<'_>,
        interrupts: &mut TurnInterrupts,
    ) -> ExecutionResult {
        let Some(mut batch) = self.batch.take() else {
            return ExecutionResult::default();
        };
        batch.stream_ended = true;
        self.pump(&mut batch, host);

        debug!(
            tools = batch.calls.len(),
            "Driving tool calls to completion."
        );

        // Register the tool interrupt handler for this phase. While
        // registered, the first Ctrl-C press is delivered to this loop; the
        // guard deregisters the handler when the calls are done.
        let (interrupt_guard, mut interrupt_rx) = signals.push_handler();

        let mut outcome = ExecutionOutcome::Completed;
        let mut stop = Stop::default();

        while !batch.settled() {
            // One client interrupt per phase. Once the calls are being
            // cancelled, the answer they give back is settled, and a second
            // reply taken here would replace the first after both were reported
            // delivered. Anything later stays queued for the phase that
            // follows: a reply becomes the next request, and a stop ends the
            // turn after this one's answer is recorded.
            let taking = !batch.cancellation.is_cancelled();

            tokio::select! {
                Some(event) = batch.receiver.recv() => {
                    self.dispatch(&mut batch, event, host);
                    self.pump(&mut batch, host);
                }
                Some(action) = interrupts.next(), if taking => {
                    // Applied even while a call's prompt is active, unlike a
                    // Ctrl-C: the press competes with the prompt for the
                    // terminal, and this does not.
                    info!(?action, "Client interrupt received during tool execution.");
                    let result = apply_tool_interrupt(
                        as_tool_interrupt(action),
                        &batch.cancellation,
                        interrupt_ui.turn_coordinator,
                    );
                    self.interrupt(&mut batch, result, &mut outcome, &mut stop, host);
                }
                Some(notice) = interrupt_rx.recv() => {
                    if batch.prompting {
                        // An active inline prompt owns the terminal; pass the
                        // interrupt down the handler stack instead of stacking
                        // the menu on top of the prompt.
                        notice.decline();
                        continue;
                    }
                    let result = handle_tool_interrupt(
                        &batch.cancellation,
                        self.is_prompting(),
                        interrupt_ui,
                        &self.interrupt_config,
                    );

                    match result {
                        // Answered by the menu: clear the ladder so the next
                        // press opens it again instead of bypassing it.
                        ToolInterruptResult::Continue
                        | ToolInterruptResult::Restart
                        | ToolInterruptResult::Cancelled { .. } => notice.handled(),

                        // A pending tool prompt owns this press; hand it to
                        // the next handler down with the ladder intact.
                        ToolInterruptResult::Declined => notice.decline(),

                        // An escalation is the user asking to get past the
                        // menu, and a menu that could not run answered
                        // nothing. Both leave the press on the ladder so it
                        // still gets the user out.
                        ToolInterruptResult::Escalate | ToolInterruptResult::PromptFailed => {}
                    }

                    self.interrupt(&mut batch, result, &mut outcome, &mut stop, host);
                }
            }
        }

        // Deregister the tool interrupt handler.
        drop(interrupt_guard);

        if batch.progress {
            host.renderer.clear_progress();
        }

        let reviews = self.collect(batch, &stop);
        ExecutionResult { reviews, outcome }
    }

    /// Act on what the tool interrupt menu decided.
    fn interrupt(
        &mut self,
        batch: &mut Batch,
        result: ToolInterruptResult,
        outcome: &mut ExecutionOutcome,
        stop: &mut Stop,
        host: &mut Host<'_>,
    ) {
        match result {
            // Either the user chose to keep waiting, or the menu could not be
            // shown and nothing happened. A declined press was already handed
            // down the stack.
            ToolInterruptResult::Continue
            | ToolInterruptResult::PromptFailed
            | ToolInterruptResult::Declined => {}
            ToolInterruptResult::Restart => {
                // Hold each call's service-side invocation open before
                // cancelling the Host workers, so the re-preparation that
                // follows continues the same logical calls instead of
                // submitting new ones.
                for call in &batch.calls {
                    call.executor.pause_for_restart();
                }
                batch.cancellation.cancel();
                self.abandon_parked(batch, host);
                outcome.upgrade(ExecutionOutcome::Restart);
            }
            ToolInterruptResult::Cancelled { response, exit } => {
                stop.cancelled = batch
                    .calls
                    .iter()
                    .enumerate()
                    .filter(|(_, call)| call.review.is_none())
                    .map(|(index, _)| index)
                    .collect();
                // Hold each unfinished call open before cancelling the Host
                // workers, so the cancellation response recorded below is
                // what its MCP caller receives. An agent that owns the call
                // builds its transcript from that, not from the conversation.
                for index in &stop.cancelled {
                    batch.calls[*index].executor.hold_for_response();
                }
                batch.cancellation.cancel();
                self.abandon_parked(batch, host);
                stop.message = response;
                if exit {
                    outcome.upgrade(ExecutionOutcome::Stopped);
                }
            }
            // The menu itself was cancelled with Ctrl-C: the tools are already
            // cancelled; surface the escalation so the turn loop begins a
            // graceful shutdown.
            ToolInterruptResult::Escalate => {
                self.abandon_parked(batch, host);
                outcome.upgrade(ExecutionOutcome::Escalated);
            }
        }
    }

    /// What the Host settled on for each call, with the cancelled ones answered
    /// by their cancellation response.
    fn collect(&self, batch: Batch, stop: &Stop) -> IndexMap<String, Review> {
        let mut reviews: IndexMap<String, Review> = batch
            .unavailable
            .into_iter()
            .map(|response| (response.id.clone(), Review::unchanged(response)))
            .collect();

        for (index, call) in batch.calls.into_iter().enumerate() {
            let Some(mut review) = call.review else {
                continue;
            };
            if stop.cancelled.contains(&index) {
                review.response.result = Ok(if let Some(msg) = &stop.message {
                    format!("Tool run cancelled by user with a custom message:\n\n{msg}")
                } else {
                    // No custom message: each cancelled tool answers with its
                    // configured cancellation response.
                    self.cancellation_response(&call.tool_name)
                });
                // The cancellation message stands in for whatever the tool
                // would have produced.
                review.edited = true;
            }
            reviews.insert(call.tool_id, review);
        }

        reviews
    }

    /// Advance the call `event` is about.
    fn dispatch(&mut self, batch: &mut Batch, event: ToolEvent, host: &mut Host<'_>) {
        match event {
            ToolEvent::Step { call, result } => {
                // A call settled while its step was in flight has its response;
                // whatever the step reached, acknowledging the call releases it.
                if matches!(batch.calls[call].wait, Wait::Done) {
                    return;
                }
                self.on_step(batch, call, result, host);
            }
            ToolEvent::Permission { call, result } => {
                batch.prompting = false;
                self.on_permission(batch, call, result, host);
            }
            ToolEvent::Answered {
                call,
                inquiry_id,
                question_id,
                answer,
                persist_level,
                redact,
            } => {
                batch.prompting = false;
                if batch.calls[call].question.take().is_none() {
                    return;
                }
                // A secret answer is persisted as `Redacted`, and never enters
                // the turn-answer cache.
                if redact {
                    Self::record_inquiry_redacted(host.conv, &inquiry_id);
                } else {
                    Self::record_inquiry_answer(host.conv, &inquiry_id, &answer);
                    if persist_level == PersistLevel::Turn {
                        let tool_name = &batch.calls[call].tool_name;
                        host.turn_state
                            .remember_answer(tool_name, &question_id, answer.clone());
                    }
                }
                self.answer(batch, call, question_id, answer);
            }
            ToolEvent::Unanswered {
                call,
                inquiry_id,
                reason,
            } => {
                batch.prompting = false;
                if batch.calls[call].question.take().is_none() {
                    return;
                }
                // A user cancellation (Esc / Ctrl-C / EOF at the prompt)
                // completes the tool benignly; a prompt failure is a tool-level
                // error.
                let result = Self::cancelled_input_result(&reason);
                Self::record_inquiry_cancelled(host.conv, &inquiry_id, reason);
                let id = batch.calls[call].tool_id.clone();
                let response = ToolCallResponse { id, result };
                self.settle(batch, call, Review::replaced(response), host);
            }
            ToolEvent::Inquired {
                call,
                inquiry_id,
                question_id,
                question_text,
                result,
            } => {
                // A withdrawn question was already closed.
                let open = batch.calls[call]
                    .question
                    .take_if(|open| open.inquiry_id == inquiry_id);
                if open.is_none() {
                    return;
                }
                match result {
                    Ok(answer) => {
                        Self::record_inquiry_answer(host.conv, &inquiry_id, &answer);
                        self.answer(batch, call, question_id, answer);
                    }
                    Err(error) => {
                        Self::record_inquiry_cancelled(
                            host.conv,
                            &inquiry_id,
                            Self::cancellation_reason(&error),
                        );
                        let Call {
                            tool_id, tool_name, ..
                        } = &batch.calls[call];
                        let response = ToolCallResponse {
                            id: tool_id.clone(),
                            result: Err(Self::inquiry_failure(tool_name, &question_text, &error)),
                        };
                        self.settle(batch, call, Review::replaced(response), host);
                    }
                }
            }
            ToolEvent::Reviewed { call, review } => {
                batch.prompting = false;
                let tool_name = batch.calls[call].tool_name.clone();
                host.renderer.focus(&batch.calls[call].tool_id);
                self.render_result(&tool_name, &review.response, host.renderer);
                self.settle(batch, call, review, host);
            }
        }
    }

    /// Take what one executor step reached.
    fn on_step(
        &mut self,
        batch: &mut Batch,
        index: usize,
        result: ExecutorResult,
        host: &mut Host<'_>,
    ) {
        let call = &mut batch.calls[index];
        let released = call.released;
        let tool_id = call.tool_id.clone();
        match result {
            ExecutorResult::AwaitingAdmission => call.wait = Wait::Admission,
            ExecutorResult::AwaitingRelease => {
                self.announce(batch, index, host);
                batch.calls[index].wait = Wait::Release;
            }
            ExecutorResult::NeedsInput {
                question,
                source,
                accumulated_answers,
                ..
            } => {
                call.answers = accumulated_answers;
                self.route(batch, index, question, source, host);
            }
            ExecutorResult::Completed(response) if released => {
                self.deliver(batch, index, response, host);
            }
            // Settled before it ran: skipped by configuration, refused by the
            // service, or a formatter that failed.
            ExecutorResult::Completed(response) => {
                self.settle(batch, index, Review::unchanged(response), host);
            }
            ExecutorResult::Failed(error) if released => {
                self.record_lost_call(batch, index, &error, false, host);
            }
            ExecutorResult::OutcomeUnknown(error) if released => {
                self.record_lost_call(batch, index, &error, true, host);
            }
            ExecutorResult::Failed(error) | ExecutorResult::OutcomeUnknown(error) => {
                let response = ToolCallResponse {
                    id: tool_id,
                    result: Err(error.to_string()),
                };
                self.settle(batch, index, Review::unchanged(response), host);
            }
        }
    }

    /// Take the user's answer to the call's approval prompt.
    fn on_permission(
        &mut self,
        batch: &mut Batch,
        index: usize,
        result: Result<PermissionResult, String>,
        host: &mut Host<'_>,
    ) {
        let call = &mut batch.calls[index];
        let Wait::Approval(info) = std::mem::replace(&mut call.wait, Wait::Step) else {
            return;
        };
        let executor = call.executor.clone();
        match self.apply_permission_result(result, &info, host.turn_state, executor.as_ref()) {
            Ok(()) => {
                let call = &mut batch.calls[index];
                call.decided = true;
                // If `e` changed the arguments, what was drawn before the
                // prompt describes a call that no longer exists, so the call
                // is drawn again once admitted.
                if call
                    .pre_render
                    .as_ref()
                    .is_some_and(|pre| pre.arguments != executor.arguments())
                {
                    call.pre_render = None;
                }
                Self::spawn_step(batch, index, Step::Approve);
            }
            Err(response) => self.settle(batch, index, Review::unchanged(response), host),
        }
    }

    /// Hand the terminal to whatever may use it next, and release the calls
    /// once they are all decided.
    fn pump(&mut self, batch: &mut Batch, host: &mut Host<'_>) {
        loop {
            if batch.stream_ended
                && !batch.released
                && batch
                    .calls
                    .iter()
                    .all(|call| matches!(call.wait, Wait::Release | Wait::Done))
            {
                self.release(batch, host);
            }

            if batch.prompting {
                return;
            }

            let cursor = batch.cursor();

            // A "no" remembered for this tool settles the call before any of
            // it reaches the screen: it would be shown, never asked about, and
            // never run.
            if let Some(call) = batch.calls.get(cursor)
                && !call.decided
                && host.interactive
                && call.executor.needs_permission()
                && host.turn_state.remembered_permission(&call.tool_name) == Some(false)
            {
                let response = Self::remembered_skip(&call.tool_id);
                self.settle(batch, cursor, Review::unchanged(response), host);
                continue;
            }

            let servable = batch.prompts.iter().position(|prompt| {
                let call = prompt.call();
                call == cursor || batch.calls[call].decided
            });
            if let Some(position) = servable
                && let Some(prompt) = batch.prompts.remove(position)
            {
                self.serve(batch, prompt, host);
                continue;
            }

            if batch
                .calls
                .get(cursor)
                .is_some_and(|call| matches!(call.wait, Wait::Admission))
            {
                self.decide(batch, cursor, host);
                continue;
            }

            return;
        }
    }

    /// Decide whether the call at the front runs: approve it, settle it, or put
    /// it up for approval.
    fn decide(&mut self, batch: &mut Batch, index: usize, host: &mut Host<'_>) {
        let executor = batch.calls[index].executor.clone();
        match self.decide_permission(executor.as_ref(), host.interactive, host.turn_state) {
            PermissionDecision::Approved => {
                batch.calls[index].decided = true;
                Self::spawn_step(batch, index, Step::Approve);
            }
            PermissionDecision::Skipped(response) => {
                self.settle(batch, index, Review::unchanged(response), host);
            }
            PermissionDecision::NeedsPrompt(info) => {
                self.set_tool_state(&info.tool_id, ToolCallState::AwaitingPermission);
                // A tool call reached from a reasoning block sits inside that
                // block's shading, and a prompt is a visual row like any other
                // (RFD 095).
                host.renderer.focus(&info.tool_id);
                host.printer
                    .set_prompt_background(host.renderer.current_region());
                // The prompt is on screen even when nothing is drawn above it.
                host.renderer.mark_drawn();

                // Drawn before the prompt so the user sees the call (not raw
                // arguments) when deciding. Built-in parameter styles always
                // draw; Custom formatters only once the service ran them.
                let call = &mut batch.calls[index];
                call.pre_render = match self.pre_render_for_prompt(executor.as_ref(), host.renderer)
                {
                    PreRender::Ready(content) => Some(PreRendered {
                        content,
                        arguments: executor.arguments(),
                    }),
                    PreRender::Deferred => None,
                };
                call.wait = Wait::Approval(info.clone());
                batch.prompting = true;

                let prompter = Arc::clone(host.prompter);
                let events = batch.events.clone();
                tokio::task::spawn_blocking(move || {
                    let result = prompter
                        .prompt_permission(&info)
                        .map_err(|error| error.to_string());
                    drop(events.send(ToolEvent::Permission {
                        call: index,
                        result,
                    }));
                });
            }
        }
    }

    /// Show the call: draw its header and description, unless they were already
    /// drawn ahead of its approval prompt.
    fn announce(&mut self, batch: &mut Batch, index: usize, host: &mut Host<'_>) {
        let call = &mut batch.calls[index];
        call.decided = true;
        call.announced = true;
        let content = if let Some(pre) = call.pre_render.take() {
            pre.content
        } else {
            host.renderer.focus(&call.tool_id);
            self.render_executor(call.executor.as_ref(), host.renderer)
        };
        if let Some(content) = content {
            self.rendered_arguments
                .insert(call.tool_id.clone(), content);
        }
    }

    /// Release every admitted call to run.
    fn release(&mut self, batch: &mut Batch, host: &mut Host<'_>) {
        batch.released = true;
        let admitted: Vec<usize> = batch
            .calls
            .iter()
            .enumerate()
            .filter(|(_, call)| matches!(call.wait, Wait::Release))
            .map(|(index, _)| index)
            .collect();
        if admitted.is_empty() {
            return;
        }

        debug!(tools = admitted.len(), "Starting tool execution.");

        // Claimed before the sinks below, not after: `StatusRegion::source`
        // copies the region it is asked of, so a sink taken from the inert
        // handle stays inert however the renderer is reassigned afterwards.
        //
        // The row ticks itself and is erased around every write the printer
        // makes, so it stays claimed for the whole execution rather than being
        // torn down and rebuilt around each event. A prompt suspends it for the
        // widget's lifetime without the coordinator arranging it.
        //
        // Progress is a terminal affordance, not a prompt, so nothing here
        // consults `interactive`: a `--no-interactive` run on a terminal still
        // shows how long a tool has been going.
        host.renderer.start_progress();
        batch.progress = true;

        for index in admitted {
            let call = &mut batch.calls[index];
            call.stderr = stderr_sink(host.renderer, &self.tools_config, &call.tool_name);
            call.released = true;
            let tool_id = call.tool_id.clone();
            self.set_tool_state(&tool_id, ToolCallState::Running);
            Self::spawn_step(batch, index, Step::Execute);
        }
    }

    /// Run one executor step, reporting back through the batch's channel.
    ///
    /// The answers are snapshotted here rather than borrowed, so the spawned
    /// task is unaffected by a later question adding to them.
    fn spawn_step(batch: &mut Batch, index: usize, step: Step) {
        let call = &mut batch.calls[index];
        call.wait = Wait::Step;
        let executor = Arc::clone(&call.executor);
        let answers = call.answers.clone();
        let stderr = call.stderr.clone();
        let token = batch.cancellation.child_token();
        let events = batch.events.clone();
        tokio::spawn(async move {
            let result = match step {
                Step::Prepare { render_arguments } => {
                    executor.prepare(render_arguments, token).await
                }
                Step::Approve => executor.approve(token).await,
                Step::Execute => executor.execute(&answers, token, stderr).await,
            };
            drop(events.send(ToolEvent::Step {
                call: index,
                result,
            }));
        });
    }

    /// Decide who answers a call's question, and set that in motion.
    ///
    /// The `InquiryRequest` is recorded before any routing decision, so every
    /// question round-trip lands on the stream however it is answered.
    /// A question answered from the turn cache or from configuration continues
    /// the call here; anything else waits for the user or the assistant.
    fn route(
        &mut self,
        batch: &mut Batch,
        index: usize,
        question: Question,
        source: InquirySource,
        host: &mut Host<'_>,
    ) {
        let call = &batch.calls[index];
        let tool_id = call.tool_id.clone();
        let tool_name = call.tool_name.clone();
        let inquiry_id =
            Self::open_inquiry(host.conv, host.turn_state, &tool_id, source, &question);
        if let Some(answer) = self.preset_answer(
            host.conv,
            host.turn_state,
            &tool_name,
            &inquiry_id,
            &question,
        ) {
            self.answer(batch, index, question.id.to_string(), answer);
            return;
        }

        let is_secret = question.answer_type == AnswerType::Secret;
        let target = self
            .question_target(&tool_name, question.id.as_str())
            .unwrap_or(QuestionTarget::User);

        info!(
            tool_name = %tool_name,
            tool_id = %tool_id,
            question_id = %question.id,
            question_text = %question.text,
            question_type = ?question.answer_type,
            target = ?target,
            interactive = host.interactive,
            "Tool question received, routing to target",
        );

        if host.interactive && target.is_user() {
            let call = &mut batch.calls[index];
            call.wait = Wait::Answer;
            call.question = Some(OpenQuestion {
                inquiry_id: inquiry_id.clone(),
                inquiry: None,
            });
            batch.prompts.push_back(Prompt::Question {
                call: index,
                question,
                inquiry_id,
            });
        } else if is_secret {
            let (reason, message) = Self::secret_refusal(&tool_name, target.is_user());
            Self::record_inquiry_cancelled(host.conv, &inquiry_id, reason);
            let response = ToolCallResponse {
                id: tool_id,
                result: Err(message),
            };
            self.settle(batch, index, Review::replaced(response), host);
        } else {
            let token = batch.cancellation.child_token();
            let call = &mut batch.calls[index];
            call.wait = Wait::Answer;
            call.question = Some(OpenQuestion {
                inquiry_id: inquiry_id.clone(),
                inquiry: Some(token.clone()),
            });
            // Nothing is on the terminal while the assistant answers, so a
            // Ctrl-C opens the interrupt menu rather than waiting for it.
            self.set_tool_state(&tool_id, ToolCallState::Running);

            let backend = Arc::clone(host.inquiry_backend);
            let events_stream = Self::paused_events(host.conv, &tool_id, &question);
            let events = batch.events.clone();
            tokio::spawn(async move {
                let result = backend
                    .inquire(
                        events_stream,
                        inquiry_id.as_str(),
                        &tool_name,
                        &question,
                        token,
                    )
                    .await;
                drop(events.send(ToolEvent::Inquired {
                    call: index,
                    inquiry_id,
                    question_id: question.id.to_string(),
                    question_text: question.text,
                    result,
                }));
            });
        }
    }

    /// Continue a call with the answer to its question.
    fn answer(&mut self, batch: &mut Batch, index: usize, question_id: String, answer: Value) {
        let call = &mut batch.calls[index];
        call.answers.insert(question_id, answer);
        let tool_id = call.tool_id.clone();
        let state = if call.released {
            ToolCallState::Running
        } else {
            ToolCallState::Queued
        };
        self.set_tool_state(&tool_id, state);
        Self::spawn_step(batch, index, Step::Execute);
    }

    /// Put a prompt on the terminal.
    fn serve(&mut self, batch: &mut Batch, prompt: Prompt, host: &mut Host<'_>) {
        match prompt {
            Prompt::Question {
                call,
                question,
                inquiry_id,
            } => {
                let tool_name = batch.calls[call].tool_name.clone();
                // An answer remembered while this waited for the terminal is
                // used rather than asking again. Secrets are never remembered.
                if question.answer_type != AnswerType::Secret
                    && let Some(answer) = host
                        .turn_state
                        .remembered_answer(&tool_name, question.id.as_str())
                        .cloned()
                {
                    Self::record_inquiry_answer(host.conv, &inquiry_id, &answer);
                    batch.calls[call].question = None;
                    self.answer(batch, call, question.id.to_string(), answer);
                    return;
                }

                let tool_id = batch.calls[call].tool_id.clone();
                self.set_tool_state(&tool_id, ToolCallState::AwaitingInput);
                host.renderer.focus(&tool_id);
                host.printer
                    .set_prompt_background(host.renderer.current_region());
                // The prompt is on screen even when nothing is drawn above it.
                host.renderer.mark_drawn();
                batch.prompting = true;

                let prompter = Arc::clone(host.prompter);
                let events = batch.events.clone();
                let question_id = question.id.to_string();
                let redact = question.answer_type == AnswerType::Secret;
                tokio::task::spawn_blocking(move || {
                    let event = match prompter.prompt_question(&question) {
                        Ok(result) => ToolEvent::Answered {
                            call,
                            inquiry_id,
                            question_id,
                            answer: result.answer,
                            persist_level: result.persist_level,
                            redact,
                        },
                        Err(error) => {
                            let reason = Self::prompt_cancellation_reason(&error);
                            // Esc/Ctrl-C is routine; only genuine prompt
                            // failures are warning-worthy. The persisted record
                            // stays coarse, so this trace is the only place the
                            // underlying error survives.
                            if reason == CancellationReason::BackendError {
                                warn!(%error, "Tool question prompt failed.");
                            }
                            ToolEvent::Unanswered {
                                call,
                                inquiry_id,
                                reason,
                            }
                        }
                    };
                    drop(events.send(event));
                });
            }
            Prompt::Review {
                call,
                response,
                mode,
            } => {
                let tool_id = batch.calls[call].tool_id.clone();
                let tool_name = batch.calls[call].tool_name.clone();
                self.set_tool_state(&tool_id, ToolCallState::AwaitingResultEdit);
                host.renderer.mark_drawn();
                batch.prompting = true;
                Self::spawn_result_mode_prompt(
                    call,
                    tool_name,
                    response,
                    mode,
                    Arc::clone(host.prompter),
                    batch.events.clone(),
                );
            }
        }
    }

    /// Hand a released call's result to its reviewer, or record it.
    fn deliver(
        &mut self,
        batch: &mut Batch,
        index: usize,
        response: ToolCallResponse,
        host: &mut Host<'_>,
    ) {
        let tool_name = batch.calls[index].tool_name.clone();
        match self.result_mode(&tool_name) {
            ResultMode::Unattended => {
                host.renderer.focus(&batch.calls[index].tool_id);
                self.render_result(&tool_name, &response, host.renderer);
                self.settle(batch, index, Review::unchanged(response), host);
            }
            // The execution service applies `result = "skip"` itself, so this
            // response is already its skip message rather than the tool's
            // output. Rendering it would announce a result the configuration
            // asked not to deliver.
            ResultMode::Skip => {
                self.settle(batch, index, Review::unchanged(response), host);
            }
            // Nobody is there to answer, so the configured prompt is skipped
            // and the result stands as the tool produced it.
            ResultMode::Ask | ResultMode::Edit if !host.interactive => {
                host.renderer.focus(&batch.calls[index].tool_id);
                self.render_result(&tool_name, &response, host.renderer);
                self.settle(batch, index, Review::unchanged(response), host);
            }
            // Both Ask and Edit prompt whenever a user is there to answer: the
            // Edit flow uses the inline widget, which does not need a
            // configured editor.
            mode => {
                batch.calls[index].wait = Wait::Review;
                batch.prompts.push_back(Prompt::Review {
                    call: index,
                    response,
                    mode,
                });
            }
        }
    }

    /// Record what the Host settled on for a call.
    ///
    /// A question still open is withdrawn: nobody needs its answer anymore.
    fn settle(&mut self, batch: &mut Batch, index: usize, review: Review, host: &mut Host<'_>) {
        self.close(batch, index, review, CancellationReason::Withdrawn, host);
    }

    /// Record what the Host settled on for a call, closing any question still
    /// open with `reason`.
    fn close(
        &mut self,
        batch: &mut Batch,
        index: usize,
        review: Review,
        reason: CancellationReason,
        host: &mut Host<'_>,
    ) {
        batch.prompts.retain(|prompt| prompt.call() != index);
        let call = &mut batch.calls[index];
        if let Some(open) = call.question.take() {
            if let Some(inquiry) = &open.inquiry {
                inquiry.cancel();
            }
            Self::record_inquiry_cancelled(host.conv, &open.inquiry_id, reason);
        }
        call.review = Some(review);
        call.wait = Wait::Done;
        call.decided = true;
        call.announced = true;
        let tool_id = call.tool_id.clone();
        self.set_tool_state(&tool_id, ToolCallState::Completed);
    }

    /// Settle every call nothing is running for, after the batch was cancelled.
    ///
    /// Work in flight reports back through its cancelled token; a call parked
    /// on a barrier or waiting for the terminal would otherwise wait forever.
    /// A question still waiting for the user was cancelled by them.
    fn abandon_parked(&mut self, batch: &mut Batch, host: &mut Host<'_>) {
        for index in 0..batch.calls.len() {
            let call = &batch.calls[index];
            let in_flight = match call.wait {
                Wait::Step => true,
                Wait::Answer => call
                    .question
                    .as_ref()
                    .is_some_and(|open| open.inquiry.is_some()),
                Wait::Done => continue,
                Wait::Admission | Wait::Approval(_) | Wait::Release | Wait::Review => false,
            };
            if in_flight {
                continue;
            }
            let response = ToolCallResponse {
                id: call.tool_id.clone(),
                result: Ok(self.cancellation_response(&call.tool_name)),
            };
            self.close(
                batch,
                index,
                Review::replaced(response),
                CancellationReason::User,
                host,
            );
        }
    }

    /// Record an `InquiryResponse::Answered` for a request recorded earlier in
    /// this turn.
    fn record_inquiry_answer(conv: &ConversationMut, inquiry_id: &InquiryId, answer: &Value) {
        conv.update_events(|events| {
            events
                .current_turn_mut()
                .add_inquiry_response(InquiryResponse::answered(
                    inquiry_id.clone(),
                    answer.clone(),
                ))
                .build()
                .expect("Invalid ConversationStream state");
        });
    }

    /// Record an `InquiryResponse::Cancelled` for a request recorded earlier in
    /// this turn, closing the pair without an answer.
    fn record_inquiry_cancelled(
        conv: &ConversationMut,
        inquiry_id: &InquiryId,
        reason: CancellationReason,
    ) {
        conv.update_events(|events| {
            events
                .current_turn_mut()
                .add_inquiry_response(InquiryResponse::Cancelled {
                    id: inquiry_id.clone(),
                    reason,
                })
                .build()
                .expect("Invalid ConversationStream state");
        });
    }

    /// Record an `InquiryResponse::Redacted` for a request recorded earlier in
    /// this turn: the tool received the answer in-memory, but the persisted
    /// record deliberately omits it.
    fn record_inquiry_redacted(conv: &ConversationMut, inquiry_id: &InquiryId) {
        conv.update_events(|events| {
            events
                .current_turn_mut()
                .add_inquiry_response(InquiryResponse::Redacted {
                    id: inquiry_id.clone(),
                })
                .build()
                .expect("Invalid ConversationStream state");
        });
    }

    /// Map an inquiry-backend error to its persisted [`CancellationReason`].
    ///
    /// `InquiryError::Cancelled` is user-initiated (the cancellation token was
    /// fired by a user action); every other variant is a genuine backend
    /// failure.
    fn cancellation_reason(error: &InquiryError) -> CancellationReason {
        match error {
            InquiryError::Cancelled => CancellationReason::User,
            InquiryError::Provider(_)
            | InquiryError::MissingStructuredData
            | InquiryError::AnswerExtraction { .. }
            | InquiryError::Other(_) => CancellationReason::BackendError,
        }
    }

    /// Map a prompter error to its persisted [`CancellationReason`].
    ///
    /// `OperationCanceled`/`OperationInterrupted` are user-initiated (Esc,
    /// Ctrl-C, or EOF at the prompt); every other error is a prompt failure.
    fn prompt_cancellation_reason(error: &Error) -> CancellationReason {
        match error {
            Error::Inquire(
                InquireError::OperationCanceled | InquireError::OperationInterrupted,
            ) => CancellationReason::User,
            _ => CancellationReason::BackendError,
        }
    }

    /// A copy of the conversation in which call `tool_id` is paused on
    /// `question`, for the assistant to answer the question from.
    fn paused_events(
        conv: &ConversationMut,
        tool_id: &str,
        question: &Question,
    ) -> ConversationStream {
        let mut events = conv.events().clone();

        // Insert a ToolCallResponse into the cloned stream so the LLM sees the
        // tool as "paused". The ID must match the original ToolCallRequest.id
        // so providers can resolve the tool name when converting events to
        // their wire format.
        events
            .current_turn_mut()
            .add_tool_call_response(ToolCallResponse {
                id: tool_id.to_owned(),
                result: Ok(format!("Tool paused: {}", question.text)),
            })
            .build()
            .expect("Invalid ConversationStream state");
        events
    }

    /// The response a call ends with when the assistant could not answer its
    /// tool's question.
    fn inquiry_failure(tool_name: &str, question_text: &str, error: &InquiryError) -> String {
        format!(
            "The tool '{tool_name}' asked a follow-up question (\"{question_text}\") that was \
             routed to a secondary assistant for resolution, but the secondary assistant failed \
             to provide a valid answer. Error: {error}. You may retry the tool call or end the \
             turn.",
        )
    }

    /// Show a finished call's result, unless the tool renders no chrome.
    fn render_result(&self, tool_name: &str, response: &ToolCallResponse, renderer: &ToolRenderer) {
        if self.is_hidden(tool_name) {
            return;
        }
        let style = self.tools_config.get(tool_name);
        let is_error = response.result.is_err();
        let (inline_results, results_file_link) = style
            .map(|config| {
                (
                    config.style().inline_results(is_error).clone(),
                    config.style().results_file_link(is_error).clone(),
                )
            })
            .unwrap_or_default();
        renderer.render_result(response, &inline_results, &results_file_link);
    }

    /// Record a call JP could not complete.
    ///
    /// The reason is JP's rather than the tool's, so the user gets the detail
    /// and the model gets only what it needs to decide what to do next.
    /// `may_have_run` is whether the tool was released before the call was
    /// lost: if it was, a retry could repeat a side effect, so the model is
    /// told to check first rather than invited to call again.
    fn record_lost_call(
        &mut self,
        batch: &mut Batch,
        index: usize,
        error: &ExecutorError,
        may_have_run: bool,
        host: &mut Host<'_>,
    ) {
        let tool_name = batch.calls[index].tool_name.clone();
        warn!(
            %error,
            tool = %tool_name,
            may_have_run,
            "Tool call could not be completed."
        );
        let message = if may_have_run {
            format!(
                "Tool '{tool_name}' may have run, but JP lost the call before its result arrived. \
                 Check whether its effects took place before calling it again."
            )
        } else {
            format!(
                "Tool '{tool_name}' was not executed: JP could not complete the call. You may \
                 retry it."
            )
        };
        let response = ToolCallResponse {
            id: batch.calls[index].tool_id.clone(),
            result: Err(message),
        };
        self.settle(batch, index, Review::replaced(response), host);
    }

    /// Record a tool's question on the current turn, and return the inquiry id
    /// its answer is recorded under.
    ///
    /// Recorded before any routing decision, so every question round-trip lands
    /// on the stream however it is answered.
    fn open_inquiry(
        conv: &ConversationMut,
        turn_state: &mut TurnState,
        tool_id: &str,
        source: InquirySource,
        question: &Question,
    ) -> InquiryId {
        let attempt = turn_state.next_inquiry_attempt(tool_id, question.id.as_str());
        let inquiry_id = InquiryId::new(inquiry::tool_call_inquiry_id(
            tool_id,
            question.id.as_str(),
            attempt,
        ));
        let inquiry_question = tool_question_to_inquiry_question(question);
        conv.update_events(|events| {
            events
                .current_turn_mut()
                .add_inquiry_request(InquiryRequest::new(
                    inquiry_id.clone(),
                    source,
                    inquiry_question,
                ))
                .build()
                .expect("Invalid ConversationStream state");
        });
        inquiry_id
    }

    /// The answer to a tool's question that needs nobody to give it: one
    /// remembered for the turn, or one configured for the tool.
    ///
    /// A found answer closes the inquiry `inquiry_id`.
    fn preset_answer(
        &self,
        conv: &ConversationMut,
        turn_state: &TurnState,
        tool_name: &str,
        inquiry_id: &InquiryId,
        question: &Question,
    ) -> Option<Value> {
        let is_secret = question.answer_type == AnswerType::Secret;

        // Secrets never enter or read the turn-answer cache.
        if !is_secret
            && let Some(answer) = turn_state.remembered_answer(tool_name, question.id.as_str())
        {
            Self::record_inquiry_answer(conv, inquiry_id, answer);
            return Some(answer.clone());
        }

        let answer = self.static_answer(tool_name, question.id.as_str())?;
        // The tool still receives the configured value in-memory; only the
        // persisted record is redacted for secrets.
        if is_secret {
            Self::record_inquiry_redacted(conv, inquiry_id);
        } else {
            Self::record_inquiry_answer(conv, inquiry_id, &answer);
        }
        Some(answer)
    }

    /// Why a secret question nobody can answer at a prompt fails its call, as
    /// the recorded cancellation reason and the message the call ends with.
    ///
    /// A secret requires a human at an interactive prompt; it is never routed
    /// to the assistant.
    fn secret_refusal(tool_name: &str, target_is_user: bool) -> (CancellationReason, String) {
        if target_is_user {
            (
                CancellationReason::NoPromptBackend,
                format!(
                    "The tool '{tool_name}' asked for a secret value, which requires an \
                     interactive prompt, but no interactive terminal is available."
                ),
            )
        } else {
            (
                CancellationReason::AssistantRoutingDenied,
                format!(
                    "The tool '{tool_name}' asked for a secret value, which must be entered by a \
                     human and cannot be routed to the assistant."
                ),
            )
        }
    }

    /// The response a call ends with when its question prompt closed without an
    /// answer.
    ///
    /// A user cancellation (Esc / Ctrl-C / EOF at the prompt) completes the
    /// tool benignly; a prompt failure is a tool-level error.
    fn cancelled_input_result(reason: &CancellationReason) -> Result<String, String> {
        match reason {
            CancellationReason::User => Ok("Tool input cancelled by user.".to_owned()),
            _ => Err("Tool input prompt failed.".to_owned()),
        }
    }

    fn spawn_result_mode_prompt(
        call: usize,
        tool_name: String,
        response: ToolCallResponse,
        result_mode: ResultMode,
        prompter: Arc<ToolPrompter>,
        events: mpsc::UnboundedSender<ToolEvent>,
    ) {
        tokio::task::spawn_blocking(move || {
            // Whether the content changed is decided here, where both the
            // offered response and the user's answer are in hand. Downstream
            // it becomes `Review::edited`, which is what lets the execution
            // service hand an unedited result back to the caller intact
            // instead of re-deriving it from the recorded text.
            let review = match result_mode {
                ResultMode::Ask => match prompter.prompt_result_confirmation(&tool_name) {
                    Ok(true) => Review::unchanged(response),
                    Ok(false) => Review::replaced(ToolCallResponse {
                        id: response.id,
                        result: Ok("Result delivery skipped by user.".to_owned()),
                    }),
                    Err(error) if error.to_string().contains("edit_requested") => {
                        Self::handle_edit_result(&prompter, response)
                    }
                    Err(_) => Review::replaced(ToolCallResponse {
                        id: response.id,
                        result: Ok("Result delivery cancelled.".to_owned()),
                    }),
                },
                ResultMode::Edit => Self::handle_edit_result(&prompter, response),
                _ => Review::unchanged(response),
            };
            drop(events.send(ToolEvent::Reviewed { call, review }));
        });
    }

    fn handle_edit_result(prompter: &ToolPrompter, response: ToolCallResponse) -> Review {
        let original = response.result.as_deref().unwrap_or_default();
        match prompter.edit_result(original) {
            // Submitting the buffer as it was offered is not an edit. Treating
            // it as one would rebuild the result from its text and drop any
            // resources, images, or structured content the tool returned.
            Ok(Some(edited)) if response.result.as_deref() == Ok(edited.as_str()) => {
                Review::unchanged(response)
            }
            Ok(Some(edited)) => Review::replaced(ToolCallResponse {
                id: response.id,
                result: Ok(edited),
            }),
            // The editor closed without a change, so the tool's own result
            // stands.
            Ok(None) => Review::unchanged(response),
            Err(_) => Review::replaced(ToolCallResponse {
                id: response.id,
                result: Ok("Result edit cancelled.".to_owned()),
            }),
        }
    }
}

impl Drop for ToolCoordinator {
    /// Stop the calls of a turn that ended before they did, such as one the
    /// user aborted while a response was streaming.
    fn drop(&mut self) {
        self.abandon();
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
