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
//! 4. After the user answers, the tool is restarted with the accumulated answers
//! 5. This continues until all tools have completed
//! 6. Results are returned in the original request order
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
//!   running). When answered, the tool is restarted with the answer.
//! - **LLM target**: The question is formatted as a response asking the LLM to
//!   re-run the tool with the answer. The tool is marked as completed.
//! - **Static answer**: Pre-populated before first execution, so the tool
//!   should never ask for this input.
//!
//! # Non-Blocking Prompts
//!
//! Interactive prompts run on a blocking thread (`spawn_blocking`) so the async
//! event loop continues processing other tool results. If multiple tools need
//! input, prompts are queued and shown sequentially.
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
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexMap;
use jp_config::conversation::tool::{QuestionTarget, ResultMode, RunMode, ToolsConfig};
use jp_conversation::{
    ConversationStream,
    event::{
        InquiryAnswerType, InquiryQuestion, InquiryRequest, InquiryResponse, InquirySource,
        SelectOption, ToolCallRequest, ToolCallResponse,
    },
};
use jp_inquire::prompt::PromptBackend;
use jp_llm::{
    ToolError,
    tool::executor::{Executor, ExecutorResult, ExecutorSource},
};
use jp_mcp::Client;
use jp_printer::Printer;
use jp_tool::{AnswerType, Question};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::{
    inquiry::{self, InquiryBackend},
    prompter::{PermissionResult, ToolPrompter, permission_inquiry_id, tool_question_inquiry_id},
    renderer::ToolRenderer,
};
use crate::cmd::query::turn::{TurnCoordinator, state::TurnState};

#[derive(Debug)]
enum ExecutionEvent {
    ToolResult {
        index: usize,
        result: ExecutorResult,
    },

    PromptAnswer {
        index: usize,
        question_id: String,
        answer: Value,
        persist_level: jp_tool::PersistLevel,
    },

    PromptCancelled {
        index: usize,
    },

    /// Result of a structured inquiry (LLM answering a tool question).
    InquiryResult {
        index: usize,
        question_id: String,
        question_text: String,
        result: Result<Value, String>,
    },

    ResultModeProcessed {
        index: usize,
        tool_id: String,
        response: ToolCallResponse,
    },

    Signal(crate::signals::SignalTo),

    ProgressTick {
        elapsed: Duration,
    },
}

#[derive(Debug)]
pub struct ExecutionResult {
    pub responses: Vec<ToolCallResponse>,
    pub restart_requested: bool,
}

struct ExecutingTool {
    executor: Arc<dyn Executor>,
    tool_id: String,
    tool_name: String,
    accumulated_answers: IndexMap<String, Value>,
}

#[derive(Debug)]
enum PendingPrompt {
    Question {
        index: usize,
        question: Question,
    },
    ResultMode {
        index: usize,
        tool_id: String,
        tool_name: String,
        response: ToolCallResponse,
        result_mode: ResultMode,
    },
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
    executor_source: Box<dyn ExecutorSource>,
    cancellation_token: CancellationToken,
}

impl ToolCoordinator {
    pub fn new(tools_config: ToolsConfig, executor_source: Box<dyn ExecutorSource>) -> Self {
        Self {
            executors: Vec::new(),
            tool_states: HashMap::new(),
            tools_config,
            executor_source,
            cancellation_token: CancellationToken::new(),
        }
    }

    pub fn is_prompting(&self) -> bool {
        self.tool_states.values().any(ToolCallState::is_prompting)
    }

    pub(crate) fn set_tool_state(&mut self, tool_id: impl Into<String>, state: ToolCallState) {
        self.tool_states.insert(tool_id.into(), state);
    }

    fn clear_tool_states(&mut self) {
        self.tool_states.clear();
    }

    pub fn parameter_style(
        &self,
        tool_name: &str,
    ) -> jp_config::conversation::tool::style::ParametersStyle {
        self.tools_config
            .get(tool_name)
            .map(|c| c.style().parameters.clone())
            .unwrap_or_default()
    }

    pub fn question_target(
        &self,
        tool_name: &str,
        question_id: &str,
    ) -> Option<jp_config::conversation::tool::QuestionTarget> {
        self.tools_config
            .get(tool_name)
            .and_then(|config| config.question_target(question_id))
    }

    pub fn static_answer(&self, tool_name: &str, question_id: &str) -> Option<serde_json::Value> {
        self.tools_config.get(tool_name).and_then(|config| {
            config
                .questions()
                .get(question_id)
                .and_then(|q| q.answer.clone())
        })
    }

    pub fn static_answers_for_tool(
        &self,
        tool_name: &str,
    ) -> indexmap::IndexMap<String, serde_json::Value> {
        let mut answers = indexmap::IndexMap::new();
        if let Some(config) = self.tools_config.get(tool_name) {
            for (question_id, question_config) in config.questions() {
                if let Some(answer) = &question_config.answer {
                    answers.insert(question_id.clone(), answer.clone());
                }
            }
        }
        answers
    }

    /// Returns pre-configured static answers for a tool's questions.
    ///
    /// These are answers set in the tool configuration (e.g. `questions.confirm.answer = true`)
    /// that bypass both user prompts and LLM inquiries.
    pub(crate) fn static_answers_for_all_questions(
        &self,
        tool_name: &str,
    ) -> IndexMap<String, Value> {
        self.static_answers_for_tool(tool_name)
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

    pub async fn prepare(
        &mut self,
        requests: Vec<ToolCallRequest>,
        mcp_client: &Client,
    ) -> Result<(), ToolError> {
        self.executors.clear();
        self.clear_tool_states();
        self.cancellation_token = CancellationToken::new();

        for (index, request) in requests.into_iter().enumerate() {
            self.tool_states
                .insert(request.id.clone(), ToolCallState::Queued);
            let executor = self
                .executor_source
                .create(request, &self.tools_config, mcp_client)
                .await?;
            self.executors.push((index, executor));
        }
        Ok(())
    }

    pub async fn run_permission_phase(
        &mut self,
        prompter: &ToolPrompter,
        mcp_client: &Client,
        is_tty: bool,
        turn_state: &mut TurnState,
    ) -> (
        Vec<(usize, Box<dyn Executor>)>,
        Vec<(usize, ToolCallResponse)>,
    ) {
        let mut approved_executors = Vec::new();
        let mut skipped_responses = Vec::new();

        for (index, executor) in std::mem::take(&mut self.executors) {
            let Some(info) = executor.permission_info() else {
                approved_executors.push((index, executor));
                continue;
            };

            if !is_tty && matches!(info.run_mode, RunMode::Ask | RunMode::Edit) {
                self.set_tool_state(&info.tool_id, ToolCallState::Running);
                approved_executors.push((index, executor));
                continue;
            }

            // Check for a persisted permission decision from earlier in this turn.
            let permission_id = permission_inquiry_id(&info.tool_name);
            let persisted = turn_state
                .persisted_inquiry_responses
                .get(&permission_id)
                .and_then(|r| r.answer.as_str())
                .map(str::to_owned);

            if let Some(ref decision) = persisted {
                match decision.as_str() {
                    "y" | "Y" => {
                        self.set_tool_state(&info.tool_id, ToolCallState::Running);
                        approved_executors.push((index, executor));
                        continue;
                    }
                    "n" | "N" => {
                        self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                        skipped_responses.push((index, ToolCallResponse {
                            id: info.tool_id.clone(),
                            result: Ok("Tool skipped by user (remembered).".to_string()),
                        }));
                        continue;
                    }
                    _ => {} // Unknown value, fall through to prompt
                }
            }

            self.set_tool_state(&info.tool_id, ToolCallState::AwaitingPermission);
            let result = prompter.prompt_permission(&info, mcp_client).await;

            match result {
                Ok(PermissionResult::Run { arguments, persist }) => {
                    if persist {
                        turn_state.persisted_inquiry_responses.insert(
                            permission_id.clone(),
                            InquiryResponse::select(permission_id, "y"),
                        );
                    }
                    let mut executor = executor;
                    executor.set_arguments(arguments);
                    self.set_tool_state(&info.tool_id, ToolCallState::Running);
                    approved_executors.push((index, executor));
                }
                Ok(PermissionResult::Skip { reason, persist }) => {
                    if persist {
                        turn_state.persisted_inquiry_responses.insert(
                            permission_id.clone(),
                            InquiryResponse::select(permission_id, "n"),
                        );
                    }
                    self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                    let msg = if let Some(r) = reason {
                        format!("Tool skipped by user: {r}")
                    } else {
                        "Tool skipped by user.".to_string()
                    };
                    skipped_responses.push((index, ToolCallResponse {
                        id: info.tool_id.clone(),
                        result: Ok(msg),
                    }));
                }
                Err(e) => {
                    self.set_tool_state(&info.tool_id, ToolCallState::Completed);
                    skipped_responses.push((index, ToolCallResponse {
                        id: info.tool_id.clone(),
                        result: Err(format!("Permission prompt failed: {e}")),
                    }));
                }
            }
        }

        (approved_executors, skipped_responses)
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    pub async fn execute_with_prompting(
        &mut self,
        executors: Vec<(usize, Box<dyn Executor>)>,
        prompter: Arc<ToolPrompter>,
        mut signal_rx: broadcast::Receiver<crate::signals::SignalTo>,
        turn_coordinator: &mut TurnCoordinator,
        turn_state: &mut TurnState,
        printer: &Printer,
        prompt_backend: &dyn PromptBackend,
        inquiry_backend: Arc<dyn InquiryBackend>,
        events: &mut ConversationStream,
        mcp_client: &Client,
        root: &Utf8Path,
        tool_renderer: &ToolRenderer,
        is_tty: bool,
    ) -> ExecutionResult {
        if executors.is_empty() {
            return ExecutionResult {
                responses: Vec::new(),
                restart_requested: false,
            };
        }

        let total_tools = executors.len();
        let cancellation_token = self.cancellation_token.clone();
        let (event_tx, mut event_rx) = mpsc::channel::<ExecutionEvent>(32);
        let mut executing_tools: HashMap<usize, ExecutingTool> = HashMap::new();
        let mut results: Vec<Option<ToolCallResponse>> = vec![None; total_tools];
        let mut pending_prompts: VecDeque<PendingPrompt> = VecDeque::new();
        let mut prompt_active = false;

        for (index, executor) in executors {
            let tool_id = executor.tool_id().to_string();
            let tool_name = executor.tool_name().to_string();
            let arguments = executor.arguments().clone();

            let accumulated_answers = self.static_answers_for_all_questions(&tool_name);

            let executor: Arc<dyn Executor> = Arc::from(executor);

            let is_hidden = self
                .tools_config
                .get(&tool_name)
                .is_some_and(|cfg| cfg.style().hidden);

            if !tool_renderer.is_rendered(&tool_id) && !is_hidden {
                tool_renderer.render_call_header(&tool_name);

                let style = self.parameter_style(&tool_name);
                if let Err(error) = tool_renderer
                    .render_arguments(&tool_name, &arguments, &style)
                    .await
                {
                    self.set_tool_state(&tool_id, ToolCallState::Completed);
                    results[index] = Some(ToolCallResponse {
                        id: tool_id,
                        result: Err(error),
                    });
                    continue;
                }
            }

            executing_tools.insert(index, ExecutingTool {
                executor: Arc::clone(&executor),
                tool_id: tool_id.clone(),
                tool_name: tool_name.clone(),
                accumulated_answers: accumulated_answers.clone(),
            });

            self.set_tool_state(&tool_id, ToolCallState::Running);

            Self::spawn_tool_execution(
                index,
                executor,
                accumulated_answers,
                mcp_client.clone(),
                root.to_path_buf(),
                cancellation_token.child_token(),
                event_tx.clone(),
            );
        }

        let signal_tx = event_tx.clone();
        tokio::spawn(async move {
            while let Ok(signal) = signal_rx.recv().await {
                if signal_tx
                    .send(ExecutionEvent::Signal(signal))
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let progress_config = tool_renderer.progress_config().clone();
        let mut progress_shown = false;

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<Duration>(1);
        let progress_token = if is_tty {
            let event_tx = event_tx.clone();
            tokio::spawn(async move {
                while let Some(elapsed) = progress_rx.recv().await {
                    if event_tx
                        .send(ExecutionEvent::ProgressTick { elapsed })
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            });

            super::renderer::spawn_tick_sender(
                progress_tx,
                progress_config.show,
                Duration::from_secs(u64::from(progress_config.delay_secs)),
                Duration::from_millis(u64::from(progress_config.interval_ms)),
            )
        } else {
            None
        };

        let mut restart_requested = false;
        let mut cancellation_response: Option<String> = None;
        let mut cancelled_indices: Vec<usize> = Vec::new();

        while let Some(event) = event_rx.recv().await {
            let was_prompting = prompt_active;

            match event {
                ExecutionEvent::ToolResult { index, result } => {
                    if progress_shown {
                        tool_renderer.clear_progress();
                        progress_shown = false;
                    }
                    let Some(tool) = executing_tools.get_mut(&index) else {
                        warn!(index, "Received ToolResult for unknown tool.");
                        continue;
                    };
                    let response = &mut results[index];
                    self.handle_tool_result(
                        result,
                        tool,
                        index,
                        response,
                        &mut pending_prompts,
                        &mut prompt_active,
                        prompter.clone(),
                        &inquiry_backend,
                        events,
                        mcp_client,
                        root,
                        &cancellation_token,
                        event_tx.clone(),
                        turn_state,
                        is_tty,
                        tool_renderer,
                    );
                }
                ExecutionEvent::PromptAnswer {
                    index,
                    question_id,
                    answer,
                    persist_level,
                } => {
                    self.handle_prompt_answer(
                        index,
                        question_id,
                        answer,
                        persist_level,
                        &mut executing_tools,
                        &mut pending_prompts,
                        &mut prompt_active,
                        prompter.clone(),
                        mcp_client,
                        root,
                        &cancellation_token,
                        event_tx.clone(),
                        turn_state,
                    );
                }
                ExecutionEvent::InquiryResult {
                    index,
                    question_id,
                    question_text,
                    result,
                } => match result {
                    Ok(answer) => {
                        if let Some(tool) = executing_tools.get_mut(&index) {
                            let id = inquiry::tool_call_inquiry_id(&tool.tool_id, &question_id);
                            events
                                .current_turn_mut()
                                .add_inquiry_response(InquiryResponse::new(id, answer.clone()))
                                .build()
                                .expect("Invalid ConversationStream state");

                            tool.accumulated_answers.insert(question_id, answer);
                            self.set_tool_state(&tool.tool_id, ToolCallState::Running);
                            Self::spawn_tool_execution(
                                index,
                                tool.executor.clone(),
                                tool.accumulated_answers.clone(),
                                mcp_client.clone(),
                                root.to_path_buf(),
                                cancellation_token.child_token(),
                                event_tx.clone(),
                            );
                        }
                    }
                    Err(error) => match executing_tools.get(&index) {
                        None => warn!(index, %error, "Received ToolResult for unknown tool."),
                        Some(tool) => {
                            self.set_tool_state(&tool.tool_id, ToolCallState::Completed);

                            results[index] = Some(ToolCallResponse {
                                id: tool.tool_id.clone(),
                                result: Err(format!(
                                    "The tool '{}' asked a follow-up question (\"{}\") that was \
                                     routed to a secondary assistant for resolution, but the \
                                     secondary assistant failed to provide a valid answer. Error: \
                                     {}. You may retry the tool call or end the turn.",
                                    tool.tool_name, question_text, error,
                                )),
                            });
                        }
                    },
                },
                ExecutionEvent::PromptCancelled { index } => {
                    self.handle_prompt_cancelled(
                        index,
                        &mut executing_tools,
                        &mut results,
                        &mut pending_prompts,
                        &mut prompt_active,
                        prompter.clone(),
                        event_tx.clone(),
                    );
                }
                ExecutionEvent::ResultModeProcessed {
                    index,
                    tool_id,
                    response,
                } => {
                    if progress_shown {
                        tool_renderer.clear_progress();
                        progress_shown = false;
                    }
                    prompt_active = false;
                    let tool_name = executing_tools
                        .get(&index)
                        .map(|t| t.tool_name.clone())
                        .unwrap_or_default();
                    let (inline_results, results_file_link) = self
                        .tools_config
                        .get(&tool_name)
                        .map(|c| {
                            (
                                c.style().inline_results.clone(),
                                c.style().results_file_link.clone(),
                            )
                        })
                        .unwrap_or_default();

                    let is_hidden = self
                        .tools_config
                        .get(&tool_name)
                        .is_some_and(|cfg| cfg.style().hidden);
                    if !is_hidden {
                        tool_renderer.render_result(&response, &inline_results, &results_file_link);
                    }

                    self.set_tool_state(&tool_id, ToolCallState::Completed);
                    results[index] = Some(response);
                    self.process_next_prompt(
                        &mut pending_prompts,
                        &mut prompt_active,
                        prompter.clone(),
                        &executing_tools,
                        event_tx.clone(),
                    );
                }
                ExecutionEvent::Signal(signal) => {
                    if !prompt_active {
                        use crate::cmd::query::interrupt::signals::{
                            ToolSignalResult, handle_tool_signal,
                        };
                        if progress_shown {
                            tool_renderer.clear_progress();
                            progress_shown = false;
                        }
                        match handle_tool_signal(
                            signal,
                            &cancellation_token,
                            turn_coordinator,
                            self.is_prompting(),
                            printer,
                            prompt_backend,
                        ) {
                            ToolSignalResult::Continue => {}
                            ToolSignalResult::Restart => {
                                restart_requested = true;
                            }
                            ToolSignalResult::Cancelled { response } => {
                                cancelled_indices = results
                                    .iter()
                                    .enumerate()
                                    .filter(|(_, r)| r.is_none())
                                    .map(|(i, _)| i)
                                    .collect();
                                cancellation_response = Some(response);
                            }
                        }
                    }
                }
                ExecutionEvent::ProgressTick { elapsed } => {
                    if !prompt_active {
                        tool_renderer.render_progress(elapsed);
                        progress_shown = true;
                    }
                }
            }

            if !was_prompting && prompt_active && progress_shown {
                tool_renderer.clear_progress();
                progress_shown = false;
            }
            if results.iter().all(Option::is_some) {
                break;
            }
        }

        if let Some(token) = progress_token {
            token.cancel();
        }
        if progress_shown {
            tool_renderer.clear_progress();
        }

        let mut responses: Vec<ToolCallResponse> = results
            .into_iter()
            .map(|r| {
                r.unwrap_or_else(|| ToolCallResponse {
                    id: "unknown".to_string(),
                    result: Err("Tool did not complete".to_string()),
                })
            })
            .collect();

        if let Some(cancel_msg) = cancellation_response {
            for &i in &cancelled_indices {
                if let Some(response) = responses.get_mut(i) {
                    response.result = Ok(cancel_msg.clone());
                }
            }
        }

        ExecutionResult {
            responses,
            restart_requested,
        }
    }

    fn spawn_tool_execution(
        index: usize,
        executor: Arc<dyn Executor>,
        answers: IndexMap<String, Value>,
        client: Client,
        root: Utf8PathBuf,
        token: CancellationToken,
        tx: mpsc::Sender<ExecutionEvent>,
    ) {
        tokio::spawn(async move {
            let result = executor.execute(&answers, &client, &root, token).await;
            let _err = tx.send(ExecutionEvent::ToolResult { index, result }).await;
        });
    }

    fn spawn_inquiry(
        index: usize,
        inquiry_id: String,
        id: String,
        question: Question,
        backend: Arc<dyn InquiryBackend>,
        mut events: ConversationStream,
        cancellation_token: CancellationToken,
        event_tx: mpsc::Sender<ExecutionEvent>,
    ) {
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
                .inquire(events, &inquiry_id, &question, cancellation_token)
                .await
                .map_err(|error| error.to_string());

            let _err = event_tx
                .send(ExecutionEvent::InquiryResult {
                    index,
                    question_id: question.id,
                    question_text: question.text,
                    result,
                })
                .await;
        });
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_lines)]
    fn handle_tool_result(
        &mut self,
        result: ExecutorResult,
        tool: &mut ExecutingTool,
        index: usize,
        tracked_response: &mut Option<ToolCallResponse>,
        pending_prompts: &mut VecDeque<PendingPrompt>,
        prompt_active: &mut bool,
        prompter: Arc<ToolPrompter>,
        inquiry_backend: &Arc<dyn InquiryBackend>,
        events: &mut ConversationStream,
        mcp_client: &Client,
        root: &Utf8Path,
        cancellation_token: &CancellationToken,
        event_tx: mpsc::Sender<ExecutionEvent>,
        turn_state: &mut TurnState,
        is_tty: bool,
        tool_renderer: &ToolRenderer,
    ) {
        match result {
            ExecutorResult::Completed(response) => {
                let (inline_results, results_file_link) = self
                    .tools_config
                    .get(&tool.tool_name)
                    .map(|c| {
                        (
                            c.style().inline_results.clone(),
                            c.style().results_file_link.clone(),
                        )
                    })
                    .unwrap_or_default();

                match self.result_mode(&tool.tool_name) {
                    ResultMode::Unattended => {
                        let is_hidden = self
                            .tools_config
                            .get(&tool.tool_name)
                            .is_some_and(|cfg| cfg.style().hidden);
                        if !is_hidden {
                            tool_renderer.render_result(
                                &response,
                                &inline_results,
                                &results_file_link,
                            );
                        }
                        self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
                        *tracked_response = Some(response);
                    }
                    ResultMode::Skip => {
                        self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
                        *tracked_response = Some(ToolCallResponse {
                            id: response.id,
                            result: Ok("Result delivery skipped by configuration.".to_string()),
                        });
                    }
                    result_mode @ (ResultMode::Ask | ResultMode::Edit) => {
                        let can_prompt =
                            is_tty && (result_mode == ResultMode::Ask || prompter.has_editor());
                        if can_prompt {
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
                                self.set_tool_state(
                                    &tool.tool_id,
                                    ToolCallState::AwaitingResultEdit,
                                );
                                Self::spawn_result_mode_prompt(
                                    index,
                                    tool.tool_id.clone(),
                                    tool.tool_name.clone(),
                                    response,
                                    result_mode,
                                    prompter,
                                    event_tx,
                                );
                            }
                        } else {
                            let is_hidden = self
                                .tools_config
                                .get(&tool.tool_name)
                                .is_some_and(|cfg| cfg.style().hidden);
                            if !is_hidden {
                                tool_renderer.render_result(
                                    &response,
                                    &inline_results,
                                    &results_file_link,
                                );
                            }
                            self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
                            *tracked_response = Some(response);
                        }
                    }
                }
            }
            ExecutorResult::NeedsInput {
                tool_id,
                tool_name,
                question,
                accumulated_answers,
            } => {
                tool.accumulated_answers = accumulated_answers.clone();

                let question_inq_id = tool_question_inquiry_id(&tool_name, &question.id);
                let persisted_answer = turn_state
                    .persisted_inquiry_responses
                    .get(&question_inq_id)
                    .map(|r| r.answer.clone());
                if let Some(answer) = persisted_answer {
                    tool.accumulated_answers.insert(question.id.clone(), answer);
                    Self::spawn_tool_execution(
                        index,
                        tool.executor.clone(),
                        tool.accumulated_answers.clone(),
                        mcp_client.clone(),
                        root.to_path_buf(),
                        cancellation_token.clone(),
                        event_tx,
                    );
                    return;
                }

                if let Some(answer) = self.static_answer(&tool_name, &question.id) {
                    tool.accumulated_answers.insert(question.id.clone(), answer);
                    Self::spawn_tool_execution(
                        index,
                        tool.executor.clone(),
                        tool.accumulated_answers.clone(),
                        mcp_client.clone(),
                        root.to_path_buf(),
                        cancellation_token.clone(),
                        event_tx,
                    );
                    return;
                }

                let target = self
                    .question_target(&tool_name, &question.id)
                    .unwrap_or(QuestionTarget::User);

                tracing::info!(
                    tool_name = %tool_name,
                    tool_id = %tool_id,
                    question_id = %question.id,
                    question_text = %question.text,
                    question_type = ?question.answer_type,
                    target = ?target,
                    is_tty = is_tty,
                    "Tool question received, routing to target",
                );

                if is_tty && target == QuestionTarget::User {
                    if *prompt_active {
                        pending_prompts.push_back(PendingPrompt::Question { index, question });
                    } else {
                        *prompt_active = true;
                        self.set_tool_state(&tool_id, ToolCallState::AwaitingInput);
                        Self::spawn_user_prompt(index, question, prompter.clone(), event_tx);
                    }
                } else {
                    // Route through the inquiry backend: either the target is
                    // explicitly `Assistant`, or the user can't be prompted
                    // (non-interactive). Record the request in the persisted
                    // stream, then spawn the async inquiry on a cloned
                    // snapshot.
                    let inquiry_id = inquiry::tool_call_inquiry_id(&tool_id, &question.id);

                    events
                        .current_turn_mut()
                        .add_inquiry_request(InquiryRequest::new(
                            inquiry_id.clone(),
                            InquirySource::tool(tool_name),
                            tool_question_to_inquiry_question(&question),
                        ))
                        .build()
                        .expect("Invalid ConversationStream state");

                    Self::spawn_inquiry(
                        index,
                        inquiry_id,
                        tool_id.clone(),
                        question,
                        Arc::clone(inquiry_backend),
                        events.clone(),
                        cancellation_token.child_token(),
                        event_tx.clone(),
                    );
                    self.set_tool_state(&tool_id, ToolCallState::AwaitingInput);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_prompt_answer(
        &mut self,
        index: usize,
        question_id: String,
        answer: Value,
        persist_level: jp_tool::PersistLevel,
        executing_tools: &mut HashMap<usize, ExecutingTool>,
        pending_prompts: &mut VecDeque<PendingPrompt>,
        prompt_active: &mut bool,
        prompter: Arc<ToolPrompter>,
        mcp_client: &Client,
        root: &Utf8Path,
        cancellation_token: &CancellationToken,
        event_tx: mpsc::Sender<ExecutionEvent>,
        turn_state: &mut TurnState,
    ) {
        *prompt_active = false;
        if let Some(tool) = executing_tools.get_mut(&index) {
            if persist_level == jp_tool::PersistLevel::Turn {
                let inq_id = tool_question_inquiry_id(&tool.tool_name, &question_id);
                turn_state
                    .persisted_inquiry_responses
                    .insert(inq_id.clone(), InquiryResponse::new(inq_id, answer.clone()));
            }
            tool.accumulated_answers.insert(question_id, answer);
            self.set_tool_state(&tool.tool_id, ToolCallState::Running);
            Self::spawn_tool_execution(
                index,
                tool.executor.clone(),
                tool.accumulated_answers.clone(),
                mcp_client.clone(),
                root.to_path_buf(),
                cancellation_token.clone(),
                event_tx.clone(),
            );
        }
        self.process_next_prompt(
            pending_prompts,
            prompt_active,
            prompter,
            executing_tools,
            event_tx,
        );
    }

    fn handle_prompt_cancelled(
        &mut self,
        index: usize,
        executing_tools: &mut HashMap<usize, ExecutingTool>,
        results: &mut [Option<ToolCallResponse>],
        pending_prompts: &mut VecDeque<PendingPrompt>,
        prompt_active: &mut bool,
        prompter: Arc<ToolPrompter>,
        event_tx: mpsc::Sender<ExecutionEvent>,
    ) {
        *prompt_active = false;
        if let Some(tool) = executing_tools.get(&index) {
            self.set_tool_state(&tool.tool_id, ToolCallState::Completed);
            results[index] = Some(ToolCallResponse {
                id: tool.tool_id.clone(),
                result: Ok("Tool input cancelled by user.".to_string()),
            });
        }
        self.process_next_prompt(
            pending_prompts,
            prompt_active,
            prompter,
            executing_tools,
            event_tx,
        );
    }

    fn spawn_user_prompt(
        index: usize,
        question: Question,
        prompter: Arc<ToolPrompter>,
        event_tx: mpsc::Sender<ExecutionEvent>,
    ) {
        let question_id = question.id.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(result) = prompter.prompt_question(&question) {
                drop(event_tx.blocking_send(ExecutionEvent::PromptAnswer {
                    index,
                    question_id,
                    answer: result.answer,
                    persist_level: result.persist_level,
                }));
            } else {
                drop(event_tx.blocking_send(ExecutionEvent::PromptCancelled { index }));
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_result_mode_prompt(
        index: usize,
        tool_id: String,
        tool_name: String,
        response: ToolCallResponse,
        result_mode: ResultMode,
        prompter: Arc<ToolPrompter>,
        event_tx: mpsc::Sender<ExecutionEvent>,
    ) {
        tokio::task::spawn_blocking(move || {
            let final_response = match result_mode {
                ResultMode::Ask => match prompter.prompt_result_confirmation(&tool_name) {
                    Ok(true) => response,
                    Ok(false) => ToolCallResponse {
                        id: response.id,
                        result: Ok("Result delivery skipped by user.".to_string()),
                    },
                    Err(e) if e.to_string().contains("edit_requested") => {
                        Self::handle_edit_result(&prompter, response)
                    }
                    Err(_) => ToolCallResponse {
                        id: response.id,
                        result: Ok("Result delivery cancelled.".to_string()),
                    },
                },
                ResultMode::Edit => Self::handle_edit_result(&prompter, response),
                _ => response,
            };
            drop(event_tx.blocking_send(ExecutionEvent::ResultModeProcessed {
                index,
                tool_id,
                response: final_response,
            }));
        });
    }

    fn handle_edit_result(prompter: &ToolPrompter, response: ToolCallResponse) -> ToolCallResponse {
        let result_str = response.result.as_ref().map_or("", |s| s.as_str());
        match prompter.edit_result(result_str) {
            Ok(Some(edited)) => ToolCallResponse {
                id: response.id,
                result: Ok(edited),
            },
            Ok(None) => response,
            Err(_) => ToolCallResponse {
                id: response.id,
                result: Ok("Result edit cancelled.".to_string()),
            },
        }
    }

    fn process_next_prompt(
        &mut self,
        pending_prompts: &mut VecDeque<PendingPrompt>,
        prompt_active: &mut bool,
        prompter: Arc<ToolPrompter>,
        executing_tools: &HashMap<usize, ExecutingTool>,
        event_tx: mpsc::Sender<ExecutionEvent>,
    ) {
        let Some(next) = pending_prompts.pop_front() else {
            return;
        };
        *prompt_active = true;
        match next {
            PendingPrompt::Question { index, question } => {
                if let Some(tool) = executing_tools.get(&index) {
                    self.set_tool_state(&tool.tool_id, ToolCallState::AwaitingInput);
                }
                Self::spawn_user_prompt(index, question, prompter, event_tx);
            }
            PendingPrompt::ResultMode {
                index,
                tool_id,
                tool_name,
                response,
                result_mode,
            } => {
                self.set_tool_state(&tool_id, ToolCallState::AwaitingResultEdit);
                Self::spawn_result_mode_prompt(
                    index,
                    tool_id,
                    tool_name,
                    response,
                    result_mode,
                    prompter.clone(),
                    event_tx.clone(),
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
