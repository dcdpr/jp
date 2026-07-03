use std::sync::{Arc, atomic::AtomicBool};

use jp_config::style::StyleConfig;
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse},
};
use jp_llm::{
    event::{Event, EventPart, FinishReason, ToolCallPart},
    event_builder::EventBuilder,
};
use jp_printer::Printer;

use crate::cmd::query::{interrupt::InterruptAction, stream::TurnView};

/// Phase of the turn state machine.
///
/// Represents the current phase within a single "turn" - the complete
/// interaction from user query to final assistant response, which may span
/// multiple LLM request-response cycles when tool calls are involved.
///
/// This is distinct from [`TurnState`], which holds data that persists across
/// phases (like retry counts and tool answers).
///
/// ```text
/// Idle → Streaming → Complete (no tools)
///            ↑     ↘ Executing
///            └────────┘
/// ```
///
/// [`TurnState`]: super::state::TurnState
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnPhase {
    /// No active turn.
    /// Waiting for `start_turn`.
    Idle,
    /// Receiving chunks from LLM.
    Streaming,
    /// Executing tool calls.
    Executing,
    /// Turn completed successfully (or stopped by user with "save").
    ///
    /// When a turn reaches this phase, the shell should persist the
    /// conversation and exit the turn loop.
    /// This includes both normal completion (LLM finished with no tool calls)
    /// and user-initiated stop (Ctrl+C → "Stop").
    Complete,
    /// Turn aborted by user (discard without saving).
    ///
    /// When a turn reaches this phase, the shell should exit the turn loop
    /// WITHOUT persisting.
    /// Any partial content is discarded.
    Aborted,
}

/// Outcome returned by [`TurnCoordinator::handle_event`].
///
/// `action` describes turn-state control flow.
/// `committed` describes the conversation event, if any, that was appended
/// while handling the input.
/// Keeping these separate avoids smuggling event-builder details into the turn
/// state machine's action enum while still letting the shell react immediately
/// to newly committed tool calls.
#[derive(Debug)]
pub struct HandleEventOutcome {
    /// The next state-machine action for the shell.
    pub action: Action,

    /// The event committed while handling the input, if the shell needs to
    /// react to it immediately.
    pub committed: CommittedEvent,
}

impl HandleEventOutcome {
    fn new(action: Action) -> Self {
        Self {
            action,
            committed: CommittedEvent::None,
        }
    }

    fn committed(action: Action, committed: CommittedEvent) -> Self {
        Self { action, committed }
    }
}

/// A committed conversation event that the shell may need to react to before
/// the current provider stream finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommittedEvent {
    /// No relevant event was committed.
    None,

    /// A [`ToolCallRequest`] was committed.
    /// The shell should run the permission/preparation pipeline immediately so
    /// prompts appear at the same streaming boundary as before.
    ToolCallRequest(ToolCallRequest),
}

/// Actions returned by the Turn Coordinator to be executed by the shell.
///
/// The coordinator is a pure state machine - it doesn't perform I/O directly.
/// Instead, it returns actions that the shell (the `handle_turn` loop)
/// executes.
#[derive(Debug)]
pub enum Action {
    /// Continue processing events (no action needed from shell).
    Continue,

    /// Transition to the executing phase.
    ///
    /// The actual list of tool calls to execute is derived from the
    /// conversation stream by [`build_execution_plan`], not carried here.
    /// The state machine only signals the phase transition; the shell derives
    /// the work from the durable source of truth.
    ///
    /// [`build_execution_plan`]: crate::cmd::query::tool::build_execution_plan
    ExecuteTools,

    /// Send tool responses back to the LLM (starts a new cycle).
    SendFollowUp,

    /// Turn finished successfully.
    Done,
}

/// Orchestrates a single turn of conversation with the LLM.
///
/// A turn consists of one or more cycles:
///
/// 1. User sends a query
/// 2. LLM streams a response (may include tool calls)
/// 3. If tool calls: execute them, send results, goto 2
/// 4. If no tool calls: turn complete
///
/// The coordinator manages:
///
/// - State transitions based on events
/// - Event accumulation via `EventBuilder`
/// - Chat rendering via [`ChatRenderer`]
/// - Pending tool calls for execution
///
/// It does NOT:
///
/// - Perform I/O (delegated to shell via `Action`)
/// - Execute tools (delegated to `ToolCoordinator`)
/// - Handle retries
///
/// [`ChatRenderer`]: crate::render::ChatRenderer
pub struct TurnCoordinator {
    state: TurnPhase,

    // Components
    event_builder: EventBuilder,
    view: TurnView,

    /// When set, emit each completed event as NDJSON.
    json_emitter: Option<JsonEmitter>,

    /// Display name to stamp onto interrupt-reply [`ChatRequest`]s for
    /// transcript attribution.
    /// `None` means the request stays unattributed.
    ///
    /// [`ChatRequest`]: jp_conversation::event::ChatRequest
    author: Option<String>,

    /// Printer for chrome notices on stderr (e.g. a non-standard finish
    /// reason).
    /// Shares the view's printer, so it is suppressed in JSON mode, matching
    /// content rendering.
    printer: Arc<Printer>,
}

impl TurnCoordinator {
    pub fn new(
        printer: Arc<Printer>,
        style: StyleConfig,
        author: Option<String>,
        assistant_name: Option<String>,
        model_id: Option<String>,
    ) -> Self {
        // In JSON mode, the renderer is unused; give it a sink so it doesn't
        // accidentally write anything.
        let (json_emitter, printer) = if printer.format().is_json() {
            (Some(JsonEmitter { printer }), Printer::sink().into())
        } else {
            (None, printer.clone())
        };

        let view = TurnView::new(printer.clone(), style, assistant_name, model_id);

        Self {
            state: TurnPhase::Idle,
            event_builder: EventBuilder::new(),
            view,
            json_emitter,
            author,
            printer,
        }
    }

    /// Start a new turn, emitting [`TurnStart`] and the user's [`ChatRequest`]
    /// into the stream in the correct order.
    ///
    /// The turn index is derived from the number of existing `TurnStart` events
    /// in the stream.
    ///
    /// Does NOT render the user's request to the terminal.
    /// The caller (e.g. `query.rs` after an editor session) is responsible for
    /// echoing the request via [`TurnView::render_user_request`] when desired
    /// — most invocations do not need an echo since the user already saw their
    /// own input on the terminal.
    ///
    /// [`TurnStart`]: jp_conversation::event::TurnStart
    pub fn start_turn(&mut self, stream: &mut ConversationStream, request: ChatRequest) {
        self.emit_json(&ConversationEvent::from(request.clone()));
        self.view.begin_turn();
        stream.start_turn(request);

        self.state = TurnPhase::Streaming;
    }

    pub fn handle_event(
        &mut self,
        stream: &mut ConversationStream,
        event: Event,
    ) -> HandleEventOutcome {
        match self.state {
            TurnPhase::Streaming => self.handle_streaming_event(stream, event),
            _ => HandleEventOutcome::new(Action::Continue),
        }
    }

    fn handle_streaming_event(
        &mut self,
        stream: &mut ConversationStream,
        event: Event,
    ) -> HandleEventOutcome {
        match event {
            Event::Part {
                index,
                part,
                metadata,
            } => {
                match &part {
                    EventPart::ToolCall(ToolCallPart::Start { .. }) => {
                        // Streaming tool calls are always visible (the
                        // hidden style only suppresses replay rendering).
                        self.view.enter_tool_call(false);
                    }
                    EventPart::Message(text) => {
                        self.view.render_chat_response(&ChatResponse::Message {
                            message: text.clone(),
                        });
                    }
                    EventPart::Reasoning(text) => {
                        self.view.render_chat_response(&ChatResponse::Reasoning {
                            reasoning: text.clone(),
                        });
                    }
                    EventPart::Structured(chunk) => {
                        self.view.render_chat_response(&ChatResponse::Structured {
                            data: serde_json::Value::String(chunk.clone()),
                        });
                    }
                    EventPart::ToolCall(ToolCallPart::ArgumentChunk(_)) => {
                        // Forwarded to EventBuilder only; no rendering.
                    }
                }

                self.event_builder.handle_part(index, part, metadata);
                HandleEventOutcome::new(Action::Continue)
            }
            Event::Flush { index, metadata } => {
                let Some(event) = self.event_builder.handle_flush(index, metadata) else {
                    return HandleEventOutcome::new(Action::Continue);
                };

                // Detect tool-call requests before consuming `event`, so
                // the shell can dispatch immediately without re-inspecting
                // the stream tail. Only tool-call requests are surfaced:
                // message, reasoning, and structured output stay on the
                // hot path without cloning.
                let committed = event
                    .as_tool_call_request()
                    .cloned()
                    .map_or(CommittedEvent::None, CommittedEvent::ToolCallRequest);
                self.push_event(stream, event);
                HandleEventOutcome::committed(Action::Continue, committed)
            }
            Event::Finished(reason) => {
                // Capture tool-call buffers about to be discarded (e.g. a tool
                // call truncated by max_tokens) so the notice can name them.
                let dropped_tools = self.event_builder.incomplete_tool_calls();

                if matches!(reason, FinishReason::Refused { .. }) {
                    // `FinishReason::Refused` contract: any partial output
                    // already streamed must be discarded. Drop the unflushed
                    // buffer instead of pushing it, and remove any assistant
                    // content already flushed into the current turn this cycle.
                    self.event_builder.clear();
                    stream.pop_while(ConversationEvent::is_chat_response);
                } else {
                    for event in self.event_builder.drain() {
                        self.push_event(stream, event);
                    }
                }

                self.view.flush();
                // The provider has stopped emitting. Switch the printer's
                // bounded-latency controller into drain mode so its
                // per-character delay holds the current pace instead of
                // slowing back up as the queue empties. The next streaming
                // cycle's first chunk resets the controller back to live
                // mode.
                self.view.signal_typewriter_drain();

                // Surface a chrome line for a non-standard finish (truncation,
                // refusal, unknown stop reason); a clean completion stays
                // silent.
                if let Some(notice) = finish_notice(&reason, &dropped_tools) {
                    self.printer.eprintln(notice);
                }

                HandleEventOutcome::new(self.transition_from_streaming(stream, reason))
            }

            // Patch is handled by the caller before reaching here; KeepAlive is
            // a liveness signal with nothing to record or render.
            Event::Patch(_) | Event::KeepAlive => HandleEventOutcome::new(Action::Continue),
        }
    }

    fn transition_from_streaming(
        &mut self,
        stream: &ConversationStream,
        _reason: FinishReason,
    ) -> Action {
        // Derive "is there work to execute?" from the stream itself, not
        // from a parallel `pending_tool_calls` cache. Any unresponded
        // tool-call request in the most recent turn means there's still
        // work; the shell will derive the actual list via
        // `build_execution_plan` once it enters the executing phase.
        if has_unresponded_tool_calls_in_current_turn(stream) {
            self.state = TurnPhase::Executing;
            return Action::ExecuteTools;
        }

        self.state = TurnPhase::Complete;
        Action::Done
    }

    /// Handle tool responses and prepare for the next cycle.
    ///
    /// Adds tool responses to the conversation stream and transitions back to
    /// `Streaming` for the follow-up LLM request.
    ///
    /// # Returns
    ///
    /// - `Action::SendFollowUp` if responses were processed and a follow-up
    ///   cycle should begin.
    ///   The caller should reset `tool_choice` to `Auto`.
    /// - `Action::Continue` if not in `Executing` phase (no-op).
    pub fn handle_tool_responses(
        &mut self,
        stream: &mut ConversationStream,
        responses: Vec<ToolCallResponse>,
    ) -> Action {
        if self.state != TurnPhase::Executing {
            return Action::Continue;
        }

        for response in responses {
            self.push_event(stream, response);
        }

        // Transition back to Streaming for the follow-up cycle
        self.state = TurnPhase::Streaming;
        Action::SendFollowUp
    }

    pub fn current_phase(&self) -> TurnPhase {
        self.state
    }

    /// Returns partial assistant content from unflushed buffers, as
    /// correctly-typed responses in stream order.
    ///
    /// Used when the user interrupts (or a retry resumes) mid-stream: the
    /// reasoning and message accumulated so far are committed so the turn
    /// resumes from where it left off, with reasoning kept as reasoning rather
    /// than folded into the assistant's answer text.
    pub fn peek_partial_events(&self) -> Vec<ChatResponse> {
        self.event_builder.peek_partial_events()
    }

    /// Resets the coordinator state back to Streaming for a new cycle.
    ///
    /// Used after handling a Continue action with prefill - the partial content
    /// has been injected into the thread, and we're ready to receive the
    /// continuation from the LLM.
    pub fn prepare_continuation(&mut self) {
        // Clear any partial buffers since we're starting fresh with prefill
        self.event_builder = EventBuilder::new();
        self.view.reset_for_continuation();
        self.state = TurnPhase::Streaming;
    }

    /// Flush the renderer's internal markdown buffer to the printer.
    ///
    /// Call this before `Printer::flush_instant()` on interrupt, so any partial
    /// content sitting in the renderer's block buffer gets queued to the
    /// printer and becomes visible before the interrupt menu appears.
    pub fn flush_renderer(&mut self) {
        self.view.flush();
    }

    /// Mark that tool calls are about to be rendered, so the next content chunk
    /// gets a blank line separator.
    pub fn transition_to_tool_call(&mut self) {
        self.view.enter_tool_call(false);
    }

    /// Wire the view's tool-separator flag to the turn's `ToolRenderer` so
    /// visible assistant content can cancel a separator owed by a tool result.
    pub fn set_tool_separator(&mut self, flag: Arc<AtomicBool>) {
        self.view.set_tool_separator(flag);
    }

    /// End the turn early: commit any partial assistant content to the stream
    /// and transition to `Complete` so the turn loop persists and exits.
    ///
    /// Used by the turn-level interrupt handler when a Ctrl-C lands between
    /// turn phases.
    pub fn complete_early(&mut self, stream: &mut ConversationStream) {
        for response in self.peek_partial_events() {
            self.push_event(stream, response);
        }

        self.state = TurnPhase::Complete;
    }

    /// Handle an interrupt action during LLM streaming.
    ///
    /// Transitions the state machine based on the user's choice from the
    /// interrupt menu.
    /// Content injection (partial content, prefill, replies) is handled here to
    /// keep the state machine self-contained.
    pub fn handle_streaming_interrupt(
        &mut self,
        action: InterruptAction,
        conversation_stream: &mut ConversationStream,
    ) -> TurnPhase {
        match action {
            // Stop and Escalate both end the turn with the partial content
            // committed; escalation additionally makes the shell begin a
            // graceful shutdown.
            InterruptAction::Stop | InterruptAction::Escalate => {
                // Inject partial content before completing
                for response in self.peek_partial_events() {
                    self.push_event(conversation_stream, response);
                }

                self.state = TurnPhase::Complete;
            }

            InterruptAction::Abort => self.state = TurnPhase::Aborted,

            InterruptAction::Continue => {
                for response in self.peek_partial_events() {
                    self.push_event(conversation_stream, response);
                }

                self.prepare_continuation();
            }

            InterruptAction::Reply(content) => {
                // Inject partial reasoning + message as assistant events first,
                // before the user's reply, so the resumed model sees its own
                // interrupted reasoning as context.
                for response in self.peek_partial_events() {
                    self.push_event(conversation_stream, response);
                }

                // Add user's reply as a new request, then render it through
                // the view so the live terminal gets the same labeled
                // user header replay would emit for this `ChatRequest`.
                // `render_user_request` also resets the assistant-header
                // gate, so the next assistant chunk will print a fresh
                // `── jp …` header.
                let request = ChatRequest {
                    content,
                    schema: None,
                    author: self.author.clone(),
                };
                self.view.render_user_request(&request);
                self.push_event(conversation_stream, request);
                self.prepare_continuation();
            }

            // Resume and tool-related actions don't change state during
            // streaming.
            InterruptAction::Resume
            | InterruptAction::ToolCancelled { .. }
            | InterruptAction::RestartTool => {}
        }

        self.state
    }

    /// Handle an interrupt action during tool execution.
    ///
    /// Tool interrupts have different semantics than streaming interrupts:
    ///
    /// - `ToolCancelled`: Cancel tools and continue with cancelled responses
    /// - `RestartTool`: Cancel and restart tool execution
    ///
    /// The actual cancellation is signaled via the `CancellationToken` which
    /// the caller must manage.
    /// This method only handles state transitions.
    /// Currently a no-op reserved for future state transitions.
    /// The shell handles cancellation via [`CancellationToken`] and restart via
    /// [`ToolInterruptResult`].
    ///
    /// [`CancellationToken`]: tokio_util::sync::CancellationToken
    /// [`ToolInterruptResult`]: crate::cmd::query::interrupt::signals::ToolInterruptResult
    #[allow(clippy::unused_self, clippy::match_same_arms)]
    pub fn handle_tool_interrupt(&mut self, action: &InterruptAction) {
        match action {
            InterruptAction::ToolCancelled { .. } | InterruptAction::RestartTool => {}
            _ => {}
        }
    }

    /// Push an event to the stream and emit as JSON if in JSON mode.
    fn push_event(&self, stream: &mut ConversationStream, event: impl Into<ConversationEvent>) {
        let event = event.into();
        self.emit_json(&event);
        stream
            .current_turn_mut()
            .add_event(event)
            .build()
            .expect("Invalid ConversationStream state");
    }

    /// Emit a conversation event as NDJSON if in JSON mode.
    fn emit_json(&self, event: &ConversationEvent) {
        if let Some(emitter) = &self.json_emitter {
            emitter.emit(event);
        }
    }
}

/// Returns `true` if the most recent turn contains any `ToolCallRequest` that
/// lacks a matching `ToolCallResponse` anywhere in the stream.
///
/// Used by the state machine to decide whether to transition into the executing
/// phase.
/// Looking across all turns for responses (rather than just the current turn)
/// is intentional: in correct operation responses always live in the same turn
/// as their request, but the cross-turn check makes the predicate robust
/// against any future code path that might commit responses to a different
/// turn.
fn has_unresponded_tool_calls_in_current_turn(stream: &ConversationStream) -> bool {
    let responded_ids: std::collections::HashSet<&str> = stream
        .iter()
        .filter_map(|e| e.event.as_tool_call_response())
        .map(|r| r.id.as_str())
        .collect();

    let Some(current_turn) = stream.iter_turns().next_back() else {
        return false;
    };

    current_turn.iter().any(|e| {
        e.event
            .as_tool_call_request()
            .is_some_and(|r| !responded_ids.contains(r.id.as_str()))
    })
}

/// Build the chrome notice for a stream that ended on a non-standard finish
/// reason, or `None` for a clean completion.
///
/// `dropped_tools` names any tool calls discarded because the stream stopped
/// mid-call (typically a max-tokens truncation).
/// Returns `None` for [`FinishReason::Completed`] and the internal
/// [`FinishReason::Retry`] signal.
fn finish_notice(reason: &FinishReason, dropped_tools: &[String]) -> Option<String> {
    match reason {
        FinishReason::Completed | FinishReason::Retry => None,
        FinishReason::MaxTokens => Some(match dropped_tools {
            [] => "Response truncated: reached the model's max output tokens. Raise `max_tokens` \
                   or narrow the request."
                .to_string(),
            [name] => format!(
                "Response truncated while building the `{name}` tool call: reached the model's \
                 max output tokens. Raise `max_tokens` or narrow the request."
            ),
            names => format!(
                "Response truncated while building tool calls ({}): reached the model's max \
                 output tokens. Raise `max_tokens` or narrow the request.",
                names.join(", ")
            ),
        }),
        FinishReason::Refused {
            category,
            explanation,
        } => {
            let category = category
                .as_deref()
                .map_or_else(String::new, |c| format!(" ({c})"));
            let explanation = explanation
                .as_deref()
                .map_or_else(|| ".".to_string(), |e| format!(": {e}"));
            Some(format!(
                "The model declined this request{category}{explanation}"
            ))
        }
        FinishReason::Other(value) => {
            let detail = value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned);
            Some(format!("Model stopped early ({detail})."))
        }
    }
}

/// Emits [`ConversationEvent`]s as NDJSON lines via the printer.
struct JsonEmitter {
    printer: Arc<Printer>,
}

impl JsonEmitter {
    fn emit(&self, event: &ConversationEvent) {
        let Ok(json) = serde_json::to_value(event) else {
            tracing::warn!("Failed to serialize event to JSON, skipping.");
            return;
        };
        let line = if self.printer.format().is_json_pretty() {
            serde_json::to_string_pretty(&json)
        } else {
            serde_json::to_string(&json)
        }
        .unwrap_or_else(|_| json.to_string());

        self.printer.println_raw(&line);
    }
}

#[cfg(test)]
#[path = "coordinator_tests.rs"]
mod tests;
