//! Tool execution coordination for the query stream pipeline.
//!
//! The [`ToolCoordinator`] manages parallel execution of multiple tool calls.
//!
//! # Execution Model
//!
//! The coordinator uses an **event-driven streaming model** where:
//!
//! 1. All tools are spawned as independent async tasks
//! 2. Results stream in as tools complete (not all at once)
//! 3. When a tool needs user input, a prompt is shown while other tools
//!    continue running in the background
//! 4. After the user answers, the tool is restarted with the accumulated
//!    answers
//! 5. This continues until all tools have completed
//! 6. Results are returned in the original request order
//!
//! <!-- end list -->
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────┐
//! │                        Event Channel                          │
//! │                                                               │
//! │  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐       │
//! │  │ Tool 1   │  │ Tool 2   │  │ Tool 3   │  │ Signal   │       │
//! │  │ (spawn)  │  │ (spawn)  │  │ (spawn)  │  │ Stream   │       │
//! │  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘       │
//! │       │             │             │             │             │
//! │       └─────────────┴─────────────┴─────────────┘             │
//! │                           │                                   │
//! │                           ▼                                   │
//! │                    ┌─────────────┐                            │
//! │                    │ Event Loop  │◄──────┐                    │
//! │                    └──────┬──────┘       │                    │
//! │                           │              │                    │
//! │         ┌─────────────────┼──────────────┼───────────┐        │
//! │         ▼                 ▼              │           ▼        │
//! │  ┌────────────┐   ┌────────────┐   ┌─────┴─────┐  ┌────────┐  │
//! │  │ Completed  │   │ NeedsInput │   │ Prompt    │  │ Signal │  │
//! │  │ → collect  │   │ (User)     │   │ Answer    │  │ Handle │  │
//! │  └────────────┘   └─────┬──────┘   │ → restart │  └────────┘  │
//! │                         │          └───────────┘              │
//! │                         ▼                                     │
//! │                  ┌─────────────────┐                          │
//! │                  │ spawn_blocking  │                          │
//! │                  │ prompt_question │───► sends PromptAnswer   │
//! │                  └─────────────────┘                          │
//! └───────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Question Handling
//!
//! When a tool returns `NeedsInput`, the coordinator checks the configuration:
//!
//! - **User target**: Prompt is shown via `spawn_blocking` (other tools keep
//!   running).
//!   When answered, the tool is restarted with the answer.
//! - **LLM target**: The question is formatted as a response asking the LLM to
//!   re-run the tool with the answer.
//!   The tool is marked as completed.
//! - **Static answer**: When the tool asks a question with a configured
//!   `QuestionConfig.answer`, the value is supplied without a prompt and the
//!   round-trip is recorded as an inquiry request/response pair.
//!
//! # Non-Blocking Prompts
//!
//! Interactive prompts run on a blocking thread (`spawn_blocking`) so the async
//! event loop continues processing other tool results.
//! If multiple tools need input, prompts are queued and shown sequentially.
//!
//! # Thread Safety
//!
//! [`TurnState`] is wrapped in [`Arc<RwLock<>>`] to allow concurrent access.
//! Each executor reads needed state, executes, then writes back results.
//!
//! # Testing
//!
//! The coordinator uses the [`Executor`] trait for tool execution.

use std::{
    collections::{HashMap, VecDeque},
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
use jp_conversation::event::{
    CancellationReason, InquiryAnswerType, InquiryId, InquiryQuestion, InquiryRequest,
    InquiryResponse, InquirySource, SelectOption, ToolCallRequest, ToolCallResponse,
};
use jp_llm::query::ToolExecution;
use jp_mcp::server::StderrSink;
use jp_printer::Printer;
use jp_tool::{AnswerType, Question};
use jp_workspace::ConversationMut;
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
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
        interrupt::signals::{InterruptUi, ToolInterruptResult, handle_tool_interrupt},
        turn::state::{PermissionCacheKey, ToolAnswerCacheKey, TurnState},
    },
    render::tool::RenderOutcome,
    signals::{InterruptNotice, SignalRouter},
};

#[derive(Debug)]
enum ExecutionEvent {
    /// A Ctrl-C press delivered to this execution phase's interrupt handler.
    Interrupt(InterruptNotice),

    ToolResult {
        index: usize,
        result: ExecutorResult,
    },

    PromptAnswer {
        index: usize,
        question_id: String,
        inquiry_id: InquiryId,
        answer: Value,
        persist_level: jp_tool::PersistLevel,
        /// Whether the persisted response must be recorded as `Redacted` (the
        /// question's answer type is `Secret`).
        redact: bool,
    },

    PromptCancelled {
        index: usize,
        inquiry_id: InquiryId,
        /// Why the prompt closed without an answer, mapped from the prompter
        /// error at the prompt site.
        reason: CancellationReason,
    },

    /// Result of a structured inquiry (LLM answering a tool question).
    InquiryResult {
        index: usize,
        inquiry_id: InquiryId,
        question_id: String,
        question_text: String,
        result: Result<Value, InquiryError>,
    },

    ResultModeProcessed {
        index: usize,
        review: Review,
    },
}

#[derive(Debug)]
pub struct ExecutionResult {
    /// What the Host settled on per tool, paired with the plan index supplied
    /// by the caller in `executors`.
    /// Indices may be sparse when the caller's plan also contains pre-resolved
    /// tools that bypass execution; merging those back into the original stream
    /// order is the caller's job.
    pub reviews: Vec<(usize, Review)>,

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
    /// downgrade the outcome — e.g. a "Stop & respond" reply clearing an
    /// earlier "Stop (cancel & exit)".
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

struct ExecutingTool {
    executor: Arc<dyn Executor>,
    tool_id: String,
    tool_name: String,
    accumulated_answers: IndexMap<String, Value>,

    /// Where this tool's stderr goes while it runs.
    ///
    /// Built once when the tool is first spawned and reused across the
    /// re-spawns an answered question triggers, so a tool that asks a question
    /// keeps feeding the same window row afterwards.
    stderr: Option<StderrSink>,
}

/// What an execution phase talks to, fixed from the moment it starts.
///
/// Every handler in the phase needs some of these and none of them changes, so
/// they travel as one borrow rather than as eight repeated parameters.
struct PhaseServices<'a> {
    /// Runs the inline prompts a question or a result review needs.
    prompter: Arc<ToolPrompter>,

    /// Answers a question routed to an assistant instead of the user.
    inquiry_backend: Arc<dyn InquiryBackend>,

    /// Where a spawned prompt, inquiry, or execution reports back to.
    event_tx: mpsc::Sender<ExecutionEvent>,

    /// Parent of every token this phase hands to a spawned task.
    cancellation_token: CancellationToken,

    /// The conversation each inquiry pair is recorded on.
    conv: &'a ConversationMut,

    /// Whether a user is there to answer a prompt at all.
    interactive: bool,
}

/// What an execution phase is keeping track of while its tools run.
///
/// Indexed by the phase's own contiguous index, not the caller's plan index:
/// see [`ToolCoordinator::execute_with_prompting`] for why the two differ.
struct PhaseState {
    /// Every call the phase started, kept for the life of the phase so an
    /// answered question can re-spawn the tool it belongs to.
    tools: HashMap<usize, ExecutingTool>,

    /// What the Host settled on per call, filled in as calls finish.
    reviews: Vec<Option<Review>>,

    /// Prompts waiting for the terminal, which one call holds at a time.
    pending_prompts: VecDeque<PendingPrompt>,

    /// Whether a prompt currently owns the terminal.
    prompt_active: bool,
}

#[derive(Debug)]
enum PendingPrompt {
    Question {
        index: usize,
        question: Question,
        inquiry_id: InquiryId,
    },
    ResultMode {
        index: usize,
        tool_id: String,
        tool_name: String,
        response: ToolCallResponse,
        result_mode: ResultMode,
    },
}

/// A question one tool asked, and what routing it needs to know.
///
/// The four travel together because answering needs all of them: the call to
/// resume, the name its configuration is keyed on, the question itself, and the
/// provenance the recorded `InquiryRequest` carries.
struct ToolQuestion {
    tool_id: String,
    tool_name: String,
    question: Question,
    source: InquirySource,
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
pub enum PermissionDecision {
    /// Tool can run immediately (unattended, persisted approval, non-TTY).
    Approved(Box<dyn Executor>),
    /// Tool should not run (persisted skip).
    Skipped(ToolCallResponse),
    /// Requires an interactive user prompt before deciding.
    NeedsPrompt {
        executor: Box<dyn Executor>,
        info: PermissionInfo,
    },
}

/// Final outcome of [`ToolCoordinator::resolve_tool_call_decision`] — the
/// per-tool permission pipeline.
///
/// This wraps the full decide → pre-render → prompt → apply → post-render
/// flow into one of three terminal states.
/// Callers map this into their own storage shape (see the streaming path in
/// `turn_loop.rs` and the batch path in
/// [`ToolCoordinator::run_permission_phase`]).
pub enum ToolCallDecision {
    /// Tool is approved and ready to be queued for execution.
    /// Includes any rendered argument content from the formatter — the caller
    /// is responsible for persisting it (typically into a `ToolCallRequest`
    /// event's metadata).
    Approved {
        executor: Box<dyn Executor>,
        rendered_arguments: Option<String>,
    },
    /// Tool was skipped: persisted "n", `RunMode::Skip`, or user declined at
    /// the prompt.
    /// The response is the synthesized skip message ready to be appended to the
    /// conversation stream.
    Skipped(ToolCallResponse),
    /// Tool failed before it could run — typically because a custom-format
    /// formatter command errored.
    /// The response tells the LLM the tool was not executed and may be retried.
    Failed(ToolCallResponse),
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
    executors: Vec<(usize, Box<dyn Executor>)>,
    tool_states: HashMap<String, ToolCallState>,
    tools_config: ToolsConfig,
    interrupt_config: ToolInterruptConfig,
    executor_source: Box<dyn ExecutorSource>,
    cancellation_token: CancellationToken,
    /// Rendered custom argument output accumulated during the permission phase.
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
            executors: Vec::new(),
            tool_states: HashMap::new(),
            tools_config,
            interrupt_config: ToolInterruptConfig::default(),
            executor_source,
            cancellation_token: CancellationToken::new(),
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
    /// Returns `(tool_call_id, rendered_content)` pairs collected during the
    /// permission phase.
    /// The caller writes these into event metadata.
    pub fn drain_rendered_arguments(&mut self) -> HashMap<String, String> {
        std::mem::take(&mut self.rendered_arguments)
    }

    pub fn is_prompting(&self) -> bool {
        self.tool_states.values().any(ToolCallState::is_prompting)
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

    fn clear_tool_states(&mut self) {
        self.tool_states.clear();
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
    ///
    /// Returns `Err` if a formatter failed — the caller should treat that as a
    /// tool failure and skip prompting.
    fn pre_render_for_prompt(
        &self,
        executor: &dyn Executor,
        tool_renderer: &ToolRenderer,
    ) -> Result<PreRender, String> {
        let name = executor.tool_name();
        if matches!(self.parameter_style(name), ParametersStyle::Custom(_))
            && executor.formatted_arguments().is_none()
        {
            // Running a user-configured shell command before the user okays
            // the tool would be surprising, so `format = "ask"` holds the
            // formatter back until admission. Built-in styles are pure and
            // have no side effects, so they always render before the prompt.
            return Ok(PreRender::Deferred);
        }

        match self.render_executor(executor, tool_renderer) {
            RenderOutcome::Rendered { content } => Ok(PreRender::Ready(content)),
            RenderOutcome::Suppressed { error } => Err(error),
        }
    }

    /// Single-tool permission pipeline.
    ///
    /// Encapsulates the full decide → pre-render → prompt → apply →
    /// post-render flow.
    /// Returns a [`ToolCallDecision`] that the caller maps to its storage
    /// shape.
    ///
    /// This is the seam where new permission-related features should land:
    /// telemetry, sandboxing decisions, alternate prompting modes — anything
    /// that needs to apply uniformly to both the streaming path (in
    /// `turn_loop.rs`) and the batch/restart path
    /// ([`Self::run_permission_phase`]).
    /// Both paths funnel through here, so changes don't drift between sites.
    ///
    /// # Pipeline steps
    ///
    /// 1. [`Self::decide_permission`] resolves the executor's run mode against
    ///    persisted answers and whether a user is available to answer.
    /// 2. If the decision is `NeedsPrompt`, pre-render the call via
    ///    [`Self::pre_render_for_prompt`] (always for built-in parameter
    ///    styles; only when `format = "unattended"` for `Custom` formatters),
    ///    then prompt the user via [`ToolPrompter::prompt_permission`], then
    ///    apply the result via [`Self::apply_permission_result`].
    ///    If the user edited the arguments at the prompt, the pre-render is
    ///    discarded so step 3 re-renders with the args that will actually
    ///    execute.
    /// 3. For approved tools, render the call (skipping if pre-rendered).
    /// 4. Return [`ToolCallDecision::Approved`], `Skipped`, or `Failed`.
    pub(crate) async fn resolve_tool_call_decision(
        &mut self,
        mut executor: Box<dyn Executor>,
        prompter: &ToolPrompter,
        interactive: bool,
        turn_state: &mut TurnState,
        tool_renderer: &ToolRenderer,
        printer: &Printer,
    ) -> ToolCallDecision {
        // A tool call reached from a reasoning block sits inside that block's
        // shading, and a prompt is a visual row like any other (RFD 095). This
        // is the one place holding both the renderer that owns the region and
        // the printer that draws the prompts, and the region is per tool, so
        // the read happens here rather than at the tool-call boundary.
        printer.set_prompt_background(tool_renderer.current_region());

        // Asking the service to format arguments for a call the user already
        // said no to would run a formatter command for output nobody sees.
        let remembered_denial = interactive
            && executor.needs_permission()
            && turn_state
                .remembered_permission_decisions
                .get(&PermissionCacheKey::new(executor.tool_name()))
                == Some(&false);
        let render_arguments = !self.is_hidden(executor.tool_name()) && !remembered_denial;
        match executor.prepare(render_arguments).await {
            Ok(Some(response)) => {
                self.set_tool_state(&response.id, ToolCallState::Completed);
                return ToolCallDecision::Skipped(response);
            }
            Ok(None) => {}
            Err(error) => {
                self.set_tool_state(executor.tool_id(), ToolCallState::Completed);
                return ToolCallDecision::Failed(ToolCallResponse {
                    id: executor.tool_id().into(),
                    result: Err(error.to_string()),
                });
            }
        }

        // Step 1: decide.
        let decision = self.decide_permission(executor, interactive, turn_state);

        // Step 2: handle prompt path. After this match, `executor` is
        // approved and `pre_rendered` is `Some(content)` if pre-rendering
        // already happened, `None` if a post-render is still needed.
        let (mut executor, pre_rendered) = match decision {
            PermissionDecision::Approved(executor) => (executor, None),
            PermissionDecision::Skipped(response) => {
                return ToolCallDecision::Skipped(response);
            }
            PermissionDecision::NeedsPrompt { executor, info } => {
                self.set_tool_state(&info.tool_id, ToolCallState::AwaitingPermission);

                // Pre-render before the prompt so the user sees the
                // rendered call (not raw arguments) when deciding.
                // Built-in parameter styles always pre-render; Custom
                // formatters are gated on `format = "unattended"`
                // because they shell out to a user-controlled command.
                let pre = match self.pre_render_for_prompt(executor.as_ref(), tool_renderer) {
                    Ok(PreRender::Ready(content)) => Some(content),
                    Ok(PreRender::Deferred) => None,
                    Err(error) => {
                        return ToolCallDecision::Failed(Self::render_failed_response(
                            info.tool_id.clone(),
                            &info.tool_name,
                            &error,
                        ));
                    }
                };

                // Snapshot the args we just rendered so we can detect a
                // user edit. If `e` changes the arguments, the pre-render
                // reflects pre-edit values and would diverge from what
                // actually executes — drop it so step 3 re-renders with
                // the post-edit args.
                let pre_edit_args = executor.arguments().clone();

                let result = prompter.prompt_permission(&info);
                match self.apply_permission_result(result, &info, turn_state, executor) {
                    Ok(executor) => {
                        let pre = if executor.arguments() == &pre_edit_args {
                            pre
                        } else {
                            None
                        };
                        (executor, pre)
                    }
                    Err(response) => return ToolCallDecision::Skipped(response),
                }
            }
        };

        if let Err(error) = executor.approve().await {
            self.set_tool_state(executor.tool_id(), ToolCallState::Completed);
            return ToolCallDecision::Failed(ToolCallResponse {
                id: executor.tool_id().into(),
                result: Err(error.to_string()),
            });
        }

        // Step 3: render. If pre-rendered, use that; otherwise render now.
        let rendered_arguments = if let Some(pre) = pre_rendered {
            pre
        } else {
            let tool_name = executor.tool_name().to_owned();
            match self.render_executor(executor.as_ref(), tool_renderer) {
                RenderOutcome::Rendered { content } => content,
                RenderOutcome::Suppressed { error } => {
                    let id = executor.tool_id().to_owned();
                    return ToolCallDecision::Failed(Self::render_failed_response(
                        id, &tool_name, &error,
                    ));
                }
            }
        };

        debug!(tool = executor.tool_name(), "Tool call decision resolved.");

        ToolCallDecision::Approved {
            executor,
            rendered_arguments,
        }
    }

    /// Render one tool call's arguments for display.
    ///
    /// A `Custom` parameter style shows what the execution service's formatter
    /// produced.
    /// The formatter is a user-configured command, so it runs once, there,
    /// under the call's access policy and cancellation token — never a second
    /// time here.
    fn render_executor(&self, executor: &dyn Executor, renderer: &ToolRenderer) -> RenderOutcome {
        let name = executor.tool_name();
        if self.is_hidden(name) {
            return RenderOutcome::Rendered { content: None };
        }
        let ParametersStyle::Custom(_) = self.parameter_style(name) else {
            return self.render_approved_tool(name, executor.arguments(), renderer);
        };
        let Some(formatted) = executor.formatted_arguments() else {
            // The service formats a call's arguments before releasing it,
            // unless the call is hidden or configured not to run. Both of
            // those are already handled, so no output here means there is no
            // call to announce: a bare header would say otherwise.
            return RenderOutcome::Rendered { content: None };
        };
        renderer.render_custom_result(name, formatted.clone().map_err(|error| error.to_string()))
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

    #[allow(dead_code)]
    pub fn cancel(&self) {
        self.cancellation_token.cancel();
    }

    /// Resets internal state for a new execution cycle.
    ///
    /// Call this when the streaming phase has already prepared executors and
    /// decided permissions, so the executing phase starts with a fresh
    /// cancellation token.
    pub fn reset_for_execution(&mut self) {
        self.cancellation_token = CancellationToken::new();
    }

    /// Prepares executors for the given tool call requests.
    ///
    /// Tools that cannot be resolved (e.g. missing from config or definitions)
    /// are returned as pre-built error responses rather than failing the entire
    /// batch.
    pub fn prepare(&mut self, requests: Vec<ToolCallRequest>) -> Vec<(usize, ToolCallResponse)> {
        self.executors.clear();
        self.clear_tool_states();
        self.cancellation_token = CancellationToken::new();

        let mut unavailable = Vec::new();
        for (index, request) in requests.into_iter().enumerate() {
            match self.prepare_one(request) {
                Ok(executor) => self.executors.push((index, executor)),
                Err(response) => unavailable.push((index, response)),
            }
        }

        unavailable
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

    /// Renders the tool call header and arguments after permission approval.
    ///
    /// Prints the header with inline-formatted arguments.
    /// A hidden tool renders nothing and still returns `Rendered`, because it
    /// also still executes.
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
    ) -> RenderOutcome {
        if self.is_hidden(tool_name) {
            return RenderOutcome::Rendered { content: None };
        }

        let style = self.parameter_style(tool_name);
        tool_renderer.render_approved(tool_name, arguments, &style)
    }

    /// Determines permission for a single tool without blocking on user input.
    ///
    /// Does NOT render any output.
    /// Rendering happens after the permission decision via
    /// [`render_approved_tool`].
    ///
    /// Returns one of:
    ///
    /// - `Approved` — tool can run immediately (unattended, persisted "y",
    ///   non-interactive)
    /// - `Skipped` — tool should not run (persisted "n")
    /// - `NeedsPrompt` — requires an interactive user prompt
    ///
    /// [`render_approved_tool`]: Self::render_approved_tool
    pub fn decide_permission(
        &mut self,
        executor: Box<dyn Executor>,
        interactive: bool,
        turn_state: &TurnState,
    ) -> PermissionDecision {
        let Some(info) = executor.permission_info() else {
            return PermissionDecision::Approved(executor);
        };

        if !interactive && matches!(info.run_mode, RunMode::Ask | RunMode::Edit) {
            self.set_tool_state(&info.tool_id, ToolCallState::Running);
            return PermissionDecision::Approved(executor);
        }

        // Check for a persisted permission decision from earlier in this turn.
        let permission_key = PermissionCacheKey::new(&info.tool_name);
        let persisted = turn_state
            .remembered_permission_decisions
            .get(&permission_key)
            .copied();

        match persisted {
            Some(true) => {
                self.set_tool_state(&info.tool_id, ToolCallState::Running);
                PermissionDecision::Approved(executor)
            }
            Some(false) => {
                self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                PermissionDecision::Skipped(ToolCallResponse {
                    id: info.tool_id.clone(),
                    result: Ok("Tool skipped by user (remembered).".to_string()),
                })
            }
            None => PermissionDecision::NeedsPrompt { executor, info },
        }
    }

    /// Applies the result of an interactive permission prompt.
    ///
    /// Call this after the user answers a prompt for a tool returned as
    /// [`PermissionDecision::NeedsPrompt`].
    pub fn apply_permission_result(
        &mut self,
        result: Result<PermissionResult, crate::error::Error>,
        info: &PermissionInfo,
        turn_state: &mut TurnState,
        mut executor: Box<dyn Executor>,
    ) -> Result<Box<dyn Executor>, ToolCallResponse> {
        let permission_key = PermissionCacheKey::new(&info.tool_name);

        match result {
            Ok(PermissionResult::Run { arguments, persist }) => {
                if persist {
                    turn_state
                        .remembered_permission_decisions
                        .insert(permission_key, true);
                }
                executor.set_arguments(arguments);
                self.set_tool_state(&info.tool_id, ToolCallState::Running);
                Ok(executor)
            }
            Ok(PermissionResult::Skip { reason, persist }) => {
                if persist {
                    turn_state
                        .remembered_permission_decisions
                        .insert(permission_key, false);
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
            Err(e) => {
                self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                Err(ToolCallResponse {
                    id: info.tool_id.clone(),
                    result: Err(format!("Permission prompt failed: {e}")),
                })
            }
        }
    }

    pub async fn run_permission_phase(
        &mut self,
        prompter: &ToolPrompter,
        interactive: bool,
        turn_state: &mut TurnState,
        tool_renderer: &ToolRenderer,
        printer: &Printer,
    ) -> (
        Vec<(usize, Box<dyn Executor>)>,
        Vec<(usize, ToolCallResponse)>,
    ) {
        let mut approved_executors = Vec::new();
        let mut skipped_responses = Vec::new();

        for (index, executor) in std::mem::take(&mut self.executors) {
            // Funnel through the unified per-tool permission pipeline. The
            // streaming path in `turn_loop.rs` uses the same call so the
            // decide → pre-render → prompt → render policy stays in one
            // place.
            let decision = self
                .resolve_tool_call_decision(
                    executor,
                    prompter,
                    interactive,
                    turn_state,
                    tool_renderer,
                    printer,
                )
                .await;

            match decision {
                ToolCallDecision::Approved {
                    executor,
                    rendered_arguments,
                } => {
                    if let Some(content) = rendered_arguments {
                        self.rendered_arguments
                            .insert(executor.tool_id().to_owned(), content);
                    }
                    approved_executors.push((index, executor));
                }
                ToolCallDecision::Skipped(response) | ToolCallDecision::Failed(response) => {
                    skipped_responses.push((index, response));
                }
            }
        }

        (approved_executors, skipped_responses)
    }

    /// Run the approved tools, answering their questions and result prompts as
    /// they arrive.
    ///
    /// `interactive` gates every question and result prompt.
    /// The elapsed-time progress row takes no parameter: it is a status region,
    /// so the printer's own terminal capability decides whether it renders.
    #[expect(clippy::too_many_lines)]
    pub async fn execute_with_prompting(
        &mut self,
        executors: Vec<(usize, Box<dyn Executor>)>,
        prompter: Arc<ToolPrompter>,
        signals: &SignalRouter,
        turn_state: &mut TurnState,
        interrupt_ui: &mut InterruptUi<'_>,
        inquiry_backend: Arc<dyn InquiryBackend>,
        conv: &ConversationMut,
        tool_renderer: &mut ToolRenderer,
        interactive: bool,
    ) -> ExecutionResult {
        if executors.is_empty() {
            return ExecutionResult {
                reviews: Vec::new(),
                outcome: ExecutionOutcome::Completed,
            };
        }

        debug!(tools = executors.len(), "Starting tool execution.");

        // Register the tool interrupt handler for this execution phase. While
        // registered, the first Ctrl-C press is delivered to this event loop;
        // the guard deregisters the handler when execution completes.
        //
        // Scoped to the conversation, so an interrupt that names one reaches the
        // handler this loop is polling. Tool execution has no timeout of its
        // own, so an unscoped handler here would leave a targeted interrupt
        // waiting for the longest tool to finish.
        let (interrupt_guard, mut interrupt_rx) = signals.push_handler_for(conv.id());

        // The caller's `index` values come from the execution plan and may
        // be sparse (e.g. when some tools in the same plan are
        // pre-resolved and don't reach this function). We can't use them
        // as offsets into a `Vec` sized to `executors.len()`, so we
        // re-base to contiguous local indices for internal bookkeeping
        // and pair each response back with its plan index on output.
        let plan_indices: Vec<usize> = executors.iter().map(|(idx, _)| *idx).collect();
        let executors: Vec<Box<dyn Executor>> =
            executors.into_iter().map(|(_, exec)| exec).collect();

        let total_tools = executors.len();
        let cancellation_token = self.cancellation_token.clone();
        let (event_tx, mut event_rx) = mpsc::channel::<ExecutionEvent>(32);
        let services = PhaseServices {
            prompter,
            inquiry_backend,
            event_tx: event_tx.clone(),
            cancellation_token: cancellation_token.clone(),
            conv,
            interactive,
        };
        let mut state = PhaseState {
            tools: HashMap::new(),
            reviews: vec![None; total_tools],
            pending_prompts: VecDeque::new(),
            prompt_active: false,
        };

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
        tool_renderer.start_progress();

        for (index, executor) in executors.into_iter().enumerate() {
            let tool_id = executor.tool_id().to_string();
            let tool_name = executor.tool_name().to_string();
            // No pre-seeding: static answers flow through the late
            // `static_answer` path so every question round-trip is recorded as
            // an inquiry pair (RFD 082).
            let accumulated_answers = IndexMap::new();

            let executor: Arc<dyn Executor> = Arc::from(executor);

            let stderr = stderr_sink(tool_renderer, &self.tools_config, &tool_name);

            let tool = ExecutingTool {
                executor,
                tool_id: tool_id.clone(),
                tool_name,
                accumulated_answers,
                stderr,
            };

            self.set_tool_state(&tool_id, ToolCallState::Running);
            Self::spawn_tool_execution(index, &tool, &services);
            state.tools.insert(index, tool);
        }

        // Forward interrupt notifications into the execution event channel.
        // The task ends when the guard drops (closing the notification
        // channel) or when the event channel closes.
        let interrupt_tx = event_tx.clone();
        tokio::spawn(async move {
            while let Some(notice) = interrupt_rx.recv().await {
                if interrupt_tx
                    .send(ExecutionEvent::Interrupt(notice))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let mut outcome = ExecutionOutcome::Completed;
        let mut tools_cancelled = false;
        let mut cancellation_message: Option<String> = None;
        let mut cancelled_indices: Vec<usize> = Vec::new();

        while let Some(event) = event_rx.recv().await {
            match event {
                ExecutionEvent::ToolResult { index, result } => {
                    if !state.tools.contains_key(&index) {
                        warn!(index, "Received ToolResult for unknown tool.");
                        continue;
                    }
                    self.handle_tool_result(
                        result,
                        index,
                        &mut state,
                        &services,
                        turn_state,
                        tool_renderer,
                    );
                }
                ExecutionEvent::PromptAnswer {
                    index,
                    question_id,
                    inquiry_id,
                    answer,
                    persist_level,
                    redact,
                } => {
                    self.handle_prompt_answer(
                        index,
                        question_id,
                        &inquiry_id,
                        answer,
                        persist_level,
                        redact,
                        &mut state,
                        &services,
                        turn_state,
                    );
                }
                ExecutionEvent::InquiryResult {
                    index,
                    inquiry_id,
                    question_id,
                    question_text,
                    result,
                } => match result {
                    Ok(answer) => {
                        // Close the recorded pair before the tool lookup, so an
                        // unknown index cannot leave the request unpaired on
                        // disk (sanitize() would drop it on the next load).
                        Self::record_inquiry_answer(services.conv, &inquiry_id, &answer);
                        if let Some(tool) = state.tools.get_mut(&index) {
                            tool.accumulated_answers.insert(question_id, answer);
                            self.set_tool_state(&tool.tool_id, ToolCallState::Running);
                            Self::spawn_tool_execution(index, tool, &services);
                        } else {
                            warn!(index, "Received InquiryResult for unknown tool.");
                        }
                    }
                    Err(error) => {
                        Self::record_inquiry_cancelled(
                            services.conv,
                            &inquiry_id,
                            Self::cancellation_reason(&error),
                        );
                        match state.tools.get(&index) {
                            None => {
                                warn!(index, %error, "Received InquiryResult for unknown tool.");
                            }
                            Some(tool) => {
                                self.set_tool_state(&tool.tool_id, ToolCallState::Completed);

                                state.reviews[index] = Some(Review::replaced(ToolCallResponse {
                                    id: tool.tool_id.clone(),
                                    result: Err(format!(
                                        "The tool '{}' asked a follow-up question (\"{}\") that \
                                         was routed to a secondary assistant for resolution, but \
                                         the secondary assistant failed to provide a valid \
                                         answer. Error: {}. You may retry the tool call or end \
                                         the turn.",
                                        tool.tool_name, question_text, error,
                                    )),
                                }));
                            }
                        }
                    }
                },
                ExecutionEvent::PromptCancelled {
                    index,
                    inquiry_id,
                    reason,
                } => {
                    self.handle_prompt_cancelled(index, &inquiry_id, reason, &mut state, &services);
                }
                ExecutionEvent::ResultModeProcessed { index, review } => {
                    state.prompt_active = false;
                    // The tool is still registered: nothing removes an entry
                    // for the life of the phase, and this index came from one.
                    if let Some(tool) = state.tools.get(&index) {
                        let tool_name = tool.tool_name.clone();
                        let tool_id = tool.tool_id.clone();
                        self.render_result(&tool_name, &review.response, tool_renderer);
                        self.set_tool_state(&tool_id, ToolCallState::Completed);
                    } else {
                        warn!(index, "Received ResultModeProcessed for unknown tool.");
                    }
                    state.reviews[index] = Some(review);
                    self.process_next_prompt(&mut state, &services);
                }
                ExecutionEvent::Interrupt(notice) => {
                    if state.prompt_active {
                        // An active inline prompt owns the terminal; pass the
                        // interrupt down the handler stack instead of stacking
                        // the menu on top of the prompt.
                        notice.decline();
                    } else {
                        let result = handle_tool_interrupt(
                            &cancellation_token,
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

                        match result {
                            // Either the user chose to keep waiting, or the menu
                            // could not be shown and nothing happened. A
                            // declined press was already handed down the stack.
                            ToolInterruptResult::Continue
                            | ToolInterruptResult::PromptFailed
                            | ToolInterruptResult::Declined => {}
                            ToolInterruptResult::Restart => {
                                // Hold each call's service-side invocation open
                                // before cancelling the Host workers, so the
                                // re-preparation that follows continues the same
                                // logical calls instead of submitting new ones.
                                for tool in state.tools.values() {
                                    tool.executor.pause_for_restart();
                                }
                                cancellation_token.cancel();
                                outcome.upgrade(ExecutionOutcome::Restart);
                            }
                            ToolInterruptResult::Cancelled { response, exit } => {
                                cancelled_indices = state
                                    .reviews
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, r)| r.is_none())
                                    .map(|(i, _)| i)
                                    .collect();
                                // Hold each unfinished call open before
                                // cancelling the Host workers, so the
                                // cancellation response recorded below is what
                                // its MCP caller receives. An agent that owns
                                // the call builds its transcript from that, not
                                // from the conversation.
                                for index in &cancelled_indices {
                                    if let Some(tool) = state.tools.get(index) {
                                        tool.executor.hold_for_response();
                                    }
                                }
                                cancellation_token.cancel();
                                tools_cancelled = true;
                                cancellation_message = response;
                                if exit {
                                    outcome.upgrade(ExecutionOutcome::Stopped);
                                }
                            }
                            // The menu itself was cancelled with Ctrl-C: the
                            // tools are already cancelled; surface the
                            // escalation so the turn loop begins a graceful
                            // shutdown.
                            ToolInterruptResult::Escalate => {
                                outcome.upgrade(ExecutionOutcome::Escalated);
                            }
                        }
                    }
                }
            }

            if state.reviews.iter().all(Option::is_some) {
                break;
            }
        }

        // Deregister the tool interrupt handler; its forwarding task exits
        // when the notification channel closes.
        drop(interrupt_guard);

        tool_renderer.clear_progress();

        let mut reviews: Vec<(usize, Review)> = plan_indices
            .into_iter()
            .zip(state.reviews.into_iter().map(|review| {
                review.unwrap_or_else(|| {
                    Review::replaced(ToolCallResponse {
                        id: "unknown".to_owned(),
                        result: Err("Tool did not complete".to_owned()),
                    })
                })
            }))
            .collect();

        if tools_cancelled {
            for &i in &cancelled_indices {
                let Some((_, review)) = reviews.get_mut(i) else {
                    continue;
                };

                review.response.result = Ok(if let Some(msg) = &cancellation_message {
                    format!("Tool run cancelled by user with a custom message:\n\n{msg}")
                } else {
                    // No custom message: each cancelled tool answers with its
                    // configured cancellation response.
                    let tool_name = state
                        .tools
                        .get(&i)
                        .map(|tool| tool.tool_name.as_str())
                        .unwrap_or_default();
                    self.cancellation_response(tool_name)
                });
                // The cancellation message stands in for whatever the tool
                // would have produced.
                review.edited = true;
            }
        }

        ExecutionResult { reviews, outcome }
    }

    /// Builds an error response for a tool whose argument rendering failed.
    ///
    /// The response tells the LLM the tool was not executed and it may retry.
    pub(crate) fn render_failed_response(
        tool_id: String,
        tool_name: &str,
        error: &str,
    ) -> ToolCallResponse {
        ToolCallResponse {
            id: tool_id,
            result: Err(format!(
                "Tool '{tool_name}' was not executed because the argument formatter failed: \
                 {error}",
            )),
        }
    }

    /// Run one attempt of a tool, reporting back through the phase's channel.
    ///
    /// The answers are snapshotted here rather than borrowed, so the spawned
    /// task is unaffected by a later question adding to them.
    fn spawn_tool_execution(index: usize, tool: &ExecutingTool, services: &PhaseServices<'_>) {
        let executor = tool.executor.clone();
        let answers = tool.accumulated_answers.clone();
        let stderr = tool.stderr.clone();
        let token = services.cancellation_token.child_token();
        let tx = services.event_tx.clone();
        tokio::spawn(async move {
            let result = executor.execute(&answers, token, stderr).await;
            let _err = tx.send(ExecutionEvent::ToolResult { index, result }).await;
        });
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

    fn spawn_inquiry(
        index: usize,
        inquiry_id: InquiryId,
        id: String,
        tool_name: String,
        question: Question,
        services: &PhaseServices<'_>,
    ) {
        let backend = Arc::clone(&services.inquiry_backend);
        let cancellation_token = services.cancellation_token.child_token();
        let event_tx = services.event_tx.clone();
        let mut events = services.conv.events().clone();

        // Insert a ToolCallResponse into the cloned stream so the LLM sees the
        // tool as "paused". The ID must match the original ToolCallRequest.id
        // so providers can resolve the tool name when converting events to
        // their wire format.
        events
            .current_turn_mut()
            .add_tool_call_response(ToolCallResponse {
                id,
                result: Ok(format!("Tool paused: {}", question.text)),
            })
            .build()
            .expect("Invalid ConversationStream state");

        tokio::spawn(async move {
            let result = backend
                .inquire(
                    events,
                    inquiry_id.as_str(),
                    &tool_name,
                    &question,
                    cancellation_token,
                )
                .await;

            let _err = event_tx
                .send(ExecutionEvent::InquiryResult {
                    index,
                    inquiry_id,
                    question_id: question.id.to_string(),
                    question_text: question.text,
                    result,
                })
                .await;
        });
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

    /// Show a call's result and record it as the content the Host settled on.
    ///
    /// This is the path for a result nobody was asked about: either the tool is
    /// configured to deliver unattended, or there is no user to ask.
    fn finish_tool_call(
        &mut self,
        tool: &ExecutingTool,
        response: ToolCallResponse,
        tracked_review: &mut Option<Review>,
        tool_renderer: &ToolRenderer,
    ) {
        self.render_result(&tool.tool_name, &response, tool_renderer);
        self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
        *tracked_review = Some(Review::unchanged(response));
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
        tool: &ExecutingTool,
        tracked_review: &mut Option<Review>,
        error: &ExecutorError,
        may_have_run: bool,
    ) {
        warn!(
            %error,
            tool = %tool.tool_name,
            may_have_run,
            "Tool call could not be completed."
        );
        let message = if may_have_run {
            format!(
                "Tool '{}' may have run, but JP lost the call before its result arrived. Check \
                 whether its effects took place before calling it again.",
                tool.tool_name
            )
        } else {
            format!(
                "Tool '{}' was not executed: JP could not complete the call. You may retry it.",
                tool.tool_name
            )
        };
        self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
        *tracked_review = Some(Review::replaced(ToolCallResponse {
            id: tool.tool_id.clone(),
            result: Err(message),
        }));
    }

    /// Take one finished attempt and decide what the phase does about it.
    ///
    /// `index` names a call the phase started; the caller checks that before
    /// dispatching here.
    fn handle_tool_result(
        &mut self,
        result: ExecutorResult,
        index: usize,
        state: &mut PhaseState,
        services: &PhaseServices<'_>,
        turn_state: &mut TurnState,
        tool_renderer: &ToolRenderer,
    ) {
        let PhaseState {
            tools,
            reviews,
            pending_prompts,
            prompt_active,
        } = state;
        let Some(tool) = tools.get_mut(&index) else {
            return;
        };
        let tracked_review = &mut reviews[index];
        match result {
            ExecutorResult::Completed(response) => {
                match self.result_mode(&tool.tool_name) {
                    ResultMode::Unattended => {
                        self.finish_tool_call(tool, response, tracked_review, tool_renderer);
                    }
                    // The execution service applies `result = "skip"` itself,
                    // so this response is already its skip message rather than
                    // the tool's output. Rendering it would announce a result
                    // the configuration asked not to deliver.
                    ResultMode::Skip => {
                        self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
                        *tracked_review = Some(Review::unchanged(response));
                    }
                    // Nobody is there to answer, so the configured prompt is
                    // skipped and the result stands as the tool produced it.
                    ResultMode::Ask | ResultMode::Edit if !services.interactive => {
                        self.finish_tool_call(tool, response, tracked_review, tool_renderer);
                    }
                    // Both Ask and Edit prompt whenever a user is there to
                    // answer: the Edit flow uses the inline widget, which does
                    // not need a configured editor.
                    result_mode => {
                        if *prompt_active {
                            pending_prompts.push_back(PendingPrompt::ResultMode {
                                index,
                                tool_id: tool.tool_id.clone(),
                                tool_name: tool.tool_name.clone(),
                                response,
                                result_mode,
                            });
                        } else {
                            *prompt_active = true;
                            self.set_tool_state(&tool.tool_id, ToolCallState::AwaitingResultEdit);
                            Self::spawn_result_mode_prompt(
                                index,
                                tool.tool_name.clone(),
                                response,
                                result_mode,
                                services,
                            );
                        }
                    }
                }
            }
            ExecutorResult::Failed(error) => {
                self.record_lost_call(tool, tracked_review, &error, false);
            }
            ExecutorResult::OutcomeUnknown(error) => {
                self.record_lost_call(tool, tracked_review, &error, true);
            }
            ExecutorResult::NeedsInput {
                tool_id,
                tool_name,
                question,
                source,
                accumulated_answers,
            } => {
                tool.accumulated_answers = accumulated_answers;
                self.route_tool_question(
                    ToolQuestion {
                        tool_id,
                        tool_name,
                        question,
                        source,
                    },
                    index,
                    tool,
                    tracked_review,
                    pending_prompts,
                    prompt_active,
                    services,
                    turn_state,
                );
            }
        }
    }

    /// Decide who answers a tool's question, and set that in motion.
    ///
    /// The `InquiryRequest` is recorded before any routing decision, so every
    /// question round-trip lands on the stream however it is answered.
    /// A question answered from the turn cache or from configuration resumes
    /// the tool here; anything else hands off to a prompt or to the assistant
    /// and resumes on a later event.
    #[expect(clippy::too_many_lines)]
    fn route_tool_question(
        &mut self,
        question: ToolQuestion,
        index: usize,
        tool: &mut ExecutingTool,
        tracked_review: &mut Option<Review>,
        pending_prompts: &mut VecDeque<PendingPrompt>,
        prompt_active: &mut bool,
        services: &PhaseServices<'_>,
        turn_state: &mut TurnState,
    ) {
        let conv = services.conv;
        let ToolQuestion {
            tool_id,
            tool_name,
            question,
            source,
        } = question;
        // Allocate the inquiry ID, incrementing the per-turn attempt counter.
        let attempt = turn_state.next_inquiry_attempt(&tool_id, question.id.as_str());
        let inquiry_id = InquiryId::new(inquiry::tool_call_inquiry_id(
            &tool_id,
            question.id.as_str(),
            attempt,
        ));
        let inquiry_question = tool_question_to_inquiry_question(&question);
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

        let is_secret = question.answer_type == AnswerType::Secret;

        // Secrets never enter or read the turn-answer cache.
        if !is_secret {
            let answer_key = ToolAnswerCacheKey::new(&tool_name, question.id.as_str());
            let persisted_answer = turn_state.remembered_tool_answers.get(&answer_key).cloned();
            if let Some(answer) = persisted_answer {
                Self::record_inquiry_answer(conv, &inquiry_id, &answer);
                tool.accumulated_answers
                    .insert(question.id.to_string(), answer);
                Self::spawn_tool_execution(index, tool, services);
                return;
            }
        }

        if let Some(answer) = self.static_answer(&tool_name, question.id.as_str()) {
            // The tool still receives the configured value in-memory;
            // only the persisted record is redacted for secrets.
            if is_secret {
                Self::record_inquiry_redacted(conv, &inquiry_id);
            } else {
                Self::record_inquiry_answer(conv, &inquiry_id, &answer);
            }
            tool.accumulated_answers
                .insert(question.id.to_string(), answer);
            Self::spawn_tool_execution(index, tool, services);
            return;
        }

        let target = self
            .question_target(&tool_name, question.id.as_str())
            .unwrap_or(QuestionTarget::User);

        tracing::info!(
            tool_name = %tool_name,
            tool_id = %tool_id,
            question_id = %question.id,
            question_text = %question.text,
            question_type = ?question.answer_type,
            target = ?target,
            interactive = services.interactive,
            "Tool question received, routing to target",
        );

        if services.interactive && target.is_user() {
            if *prompt_active {
                pending_prompts.push_back(PendingPrompt::Question {
                    index,
                    question,
                    inquiry_id,
                });
            } else {
                *prompt_active = true;
                self.set_tool_state(&tool_id, ToolCallState::AwaitingInput);
                Self::spawn_user_prompt(index, question, inquiry_id, services);
            }
        } else if is_secret {
            // A secret requires a human at an interactive prompt; it must never
            // route to the inquiry backend. Fail the tool and close the
            // recorded inquiry with the guard's reason.
            let (reason, message) = if target.is_user() {
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
                        "The tool '{tool_name}' asked for a secret value, which must be entered \
                         by a human and cannot be routed to the assistant."
                    ),
                )
            };
            Self::record_inquiry_cancelled(conv, &inquiry_id, reason);
            self.set_tool_state(&tool_id, ToolCallState::Completed);
            *tracked_review = Some(Review::replaced(ToolCallResponse {
                id: tool_id.clone(),
                result: Err(message),
            }));
        } else {
            // The `InquiryRequest` is already recorded above; spawn the
            // async inquiry on a cloned snapshot.
            Self::spawn_inquiry(
                index,
                inquiry_id,
                tool_id.clone(),
                tool_name,
                question,
                services,
            );
            self.set_tool_state(&tool_id, ToolCallState::AwaitingInput);
        }
    }

    fn handle_prompt_answer(
        &mut self,
        index: usize,
        question_id: String,
        inquiry_id: &InquiryId,
        answer: Value,
        persist_level: jp_tool::PersistLevel,
        redact: bool,
        state: &mut PhaseState,
        services: &PhaseServices<'_>,
        turn_state: &mut TurnState,
    ) {
        state.prompt_active = false;

        // Close the recorded inquiry with the user's answer; a secret answer
        // is persisted as `Redacted` and never carries the value.
        if redact {
            Self::record_inquiry_redacted(services.conv, inquiry_id);
        } else {
            Self::record_inquiry_answer(services.conv, inquiry_id, &answer);
        }

        if let Some(tool) = state.tools.get_mut(&index) {
            // Secrets never enter the turn-answer cache.
            if persist_level == jp_tool::PersistLevel::Turn && !redact {
                let answer_key = ToolAnswerCacheKey::new(&tool.tool_name, &question_id);
                turn_state
                    .remembered_tool_answers
                    .insert(answer_key, answer.clone());
            }
            tool.accumulated_answers.insert(question_id, answer);
            self.set_tool_state(&tool.tool_id, ToolCallState::Running);
            Self::spawn_tool_execution(index, tool, services);
        }
        self.process_next_prompt(state, services);
    }

    fn handle_prompt_cancelled(
        &mut self,
        index: usize,
        inquiry_id: &InquiryId,
        reason: CancellationReason,
        state: &mut PhaseState,
        services: &PhaseServices<'_>,
    ) {
        state.prompt_active = false;

        // A user cancellation (Esc / Ctrl-C / EOF at the prompt) completes the
        // tool benignly; a prompt failure is a tool-level error.
        let result = match reason {
            CancellationReason::User => Ok("Tool input cancelled by user.".to_owned()),
            _ => Err("Tool input prompt failed.".to_owned()),
        };
        Self::record_inquiry_cancelled(services.conv, inquiry_id, reason);

        if let Some(tool) = state.tools.get(&index) {
            self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
            state.reviews[index] = Some(Review::replaced(ToolCallResponse {
                id: tool.tool_id.clone(),
                result,
            }));
        }
        self.process_next_prompt(state, services);
    }

    fn spawn_user_prompt(
        index: usize,
        question: Question,
        inquiry_id: InquiryId,
        services: &PhaseServices<'_>,
    ) {
        let prompter = services.prompter.clone();
        let event_tx = services.event_tx.clone();
        let question_id = question.id.to_string();
        let redact = question.answer_type == AnswerType::Secret;
        tokio::task::spawn_blocking(move || match prompter.prompt_question(&question) {
            Ok(result) => {
                drop(event_tx.blocking_send(ExecutionEvent::PromptAnswer {
                    index,
                    question_id,
                    inquiry_id,
                    answer: result.answer,
                    persist_level: result.persist_level,
                    redact,
                }));
            }
            Err(error) => {
                let reason = Self::prompt_cancellation_reason(&error);
                // Esc/Ctrl-C is routine; only genuine prompt failures are
                // warning-worthy. The persisted record stays coarse, so this
                // trace is the only place the underlying error survives.
                if reason == CancellationReason::BackendError {
                    warn!(%error, "Tool question prompt failed.");
                }
                drop(event_tx.blocking_send(ExecutionEvent::PromptCancelled {
                    index,
                    inquiry_id,
                    reason,
                }));
            }
        });
    }

    fn spawn_result_mode_prompt(
        index: usize,
        tool_name: String,
        response: ToolCallResponse,
        result_mode: ResultMode,
        services: &PhaseServices<'_>,
    ) {
        let prompter = services.prompter.clone();
        let event_tx = services.event_tx.clone();
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
            drop(event_tx.blocking_send(ExecutionEvent::ResultModeProcessed { index, review }));
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

    /// Hand the terminal to the next waiting prompt, if there is one.
    fn process_next_prompt(&mut self, state: &mut PhaseState, services: &PhaseServices<'_>) {
        let Some(next) = state.pending_prompts.pop_front() else {
            return;
        };
        state.prompt_active = true;
        match next {
            PendingPrompt::Question {
                index,
                question,
                inquiry_id,
            } => {
                if let Some(tool) = state.tools.get(&index) {
                    self.set_tool_state(&tool.tool_id, ToolCallState::AwaitingInput);
                }
                Self::spawn_user_prompt(index, question, inquiry_id, services);
            }
            PendingPrompt::ResultMode {
                index,
                tool_id,
                tool_name,
                response,
                result_mode,
            } => {
                self.set_tool_state(&tool_id, ToolCallState::AwaitingResultEdit);
                Self::spawn_result_mode_prompt(index, tool_name, response, result_mode, services);
            }
        }
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
