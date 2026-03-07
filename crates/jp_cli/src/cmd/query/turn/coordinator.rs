use std::sync::Arc;

use jp_config::style::StyleConfig;
use jp_conversation::{
    ConversationEvent, ConversationStream, EventKind,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse},
    event_builder::EventBuilder,
};
use jp_llm::event::{Event, FinishReason};
use jp_printer::Printer;

use crate::cmd::query::{
    interrupt::InterruptAction,
    stream::{ChatResponseRenderer, StructuredRenderer},
};

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
    /// No active turn. Waiting for `start_turn`.
    Idle,
    /// Receiving chunks from LLM.
    Streaming,
    /// Executing tool calls.
    Executing,
    /// Turn completed successfully (or stopped by user with "save").
    ///
    /// When a turn reaches this phase, the shell should persist the
    /// conversation and exit the turn loop. This includes both normal
    /// completion (LLM finished with no tool calls) and user-initiated
    /// stop (Ctrl+C → "Stop").
    Complete,
    /// Turn aborted by user (discard without saving).
    ///
    /// When a turn reaches this phase, the shell should exit the turn loop
    /// WITHOUT persisting. Any partial content is discarded.
    Aborted,
}

/// Actions returned by the Turn Coordinator to be executed by the shell.
///
/// The coordinator is a pure state machine - it doesn't perform I/O directly.
/// Instead, it returns actions that the shell (the `handle_turn` loop) executes.
#[derive(Debug)]
pub enum Action {
    /// Continue processing events (no action needed from shell).
    Continue,

    /// Execute a list of tool calls via `ToolCoordinator`.
    ///
    /// The tool calls are also available via `take_pending_tool_calls()`.
    /// The field is included for debugging and testing.
    #[allow(dead_code)]
    ExecuteTools(Vec<ToolCallRequest>),

    /// Send tool responses back to the LLM (starts a new cycle).
    SendFollowUp,

    /// Turn finished successfully.
    Done,
}

/// Orchestrates a single turn of conversation with the LLM.
///
/// A turn consists of one or more cycles:
/// 1. User sends a query
/// 2. LLM streams a response (may include tool calls)
/// 3. If tool calls: execute them, send results, goto 2
/// 4. If no tool calls: turn complete
///
/// The coordinator manages:
/// - State transitions based on events
/// - Event accumulation via `EventBuilder`
/// - Chat rendering via `ChatResponseRenderer`
/// - Pending tool calls for execution
///
/// It does NOT:
/// - Perform I/O (delegated to shell via `Action`)
/// - Execute tools (delegated to `ToolCoordinator`)
/// - Handle retries
pub struct TurnCoordinator {
    state: TurnPhase,

    // Components
    event_builder: EventBuilder,
    chat_renderer: ChatResponseRenderer,
    structured_renderer: StructuredRenderer,

    /// When set, emit each completed event as NDJSON.
    json_emitter: Option<JsonEmitter>,

    // Accumulators for the current cycle
    pending_tool_calls: Vec<ToolCallRequest>,
}

impl TurnCoordinator {
    pub fn new(printer: Arc<Printer>, style: StyleConfig) -> Self {
        // In JSON mode, the renderer is unused; give it a sink so it doesn't
        // accidentally write anything.
        let (json_emitter, printer) = if printer.format().is_json() {
            (Some(JsonEmitter { printer }), Printer::sink().into())
        } else {
            (None, printer.clone())
        };

        Self {
            state: TurnPhase::Idle,
            event_builder: EventBuilder::new(),
            chat_renderer: ChatResponseRenderer::new(printer.clone(), style),
            structured_renderer: StructuredRenderer::new(printer),
            json_emitter,
            pending_tool_calls: Vec::new(),
        }
    }

    /// Start a new turn, emitting [`TurnStart`] and the user's
    /// [`ChatRequest`] into the stream in the correct order.
    ///
    /// The turn index is derived from the number of existing `TurnStart`
    /// events in the stream.
    ///
    /// [`TurnStart`]: jp_conversation::event::TurnStart
    pub fn start_turn(&mut self, stream: &mut ConversationStream, request: ChatRequest) {
        self.emit_json(&ConversationEvent::from(request.clone()));
        stream.start_turn(request);

        self.state = TurnPhase::Streaming;
    }

    pub fn handle_event(&mut self, stream: &mut ConversationStream, event: Event) -> Action {
        match self.state {
            TurnPhase::Streaming => self.handle_streaming_event(stream, event),
            _ => Action::Continue,
        }
    }

    fn handle_streaming_event(&mut self, stream: &mut ConversationStream, event: Event) -> Action {
        match event {
            Event::Part { index, event } => {
                if let EventKind::ToolCallRequest(_) = &event.kind {
                    // Flush any buffered markdown so it appears before the
                    // tool-call output rather than being delayed until after.
                    self.chat_renderer.flush();
                    self.chat_renderer.reset_content_kind();
                }

                match &event.kind {
                    EventKind::ChatResponse(
                        resp @ (ChatResponse::Message { .. } | ChatResponse::Reasoning { .. }),
                    ) => {
                        self.chat_renderer.render(resp);
                    }
                    EventKind::ChatResponse(resp @ ChatResponse::Structured { .. }) => {
                        self.structured_renderer.render_chunk(resp);
                    }
                    _ => {}
                }

                self.event_builder.handle_part(index, event);
                Action::Continue
            }
            Event::Flush { index, metadata } => {
                if let Some(event) = self.event_builder.handle_flush(index, metadata) {
                    if let Some(req) = event.as_tool_call_request() {
                        self.pending_tool_calls.push(req.clone());
                    }
                    self.push_event(stream, event);
                }

                Action::Continue
            }
            Event::Finished(reason) => {
                for event in self.event_builder.drain() {
                    self.push_event(stream, event);
                }
                self.chat_renderer.flush();
                self.structured_renderer.flush();
                self.transition_from_streaming(reason)
            }
        }
    }

    fn transition_from_streaming(&mut self, _reason: FinishReason) -> Action {
        if !self.pending_tool_calls.is_empty() {
            self.state = TurnPhase::Executing;
            let calls = self.pending_tool_calls.clone();
            return Action::ExecuteTools(calls);
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
    ///   cycle should begin. The caller should reset `tool_choice` to `Auto`.
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

    pub fn take_pending_tool_calls(&mut self) -> Vec<ToolCallRequest> {
        std::mem::take(&mut self.pending_tool_calls)
    }

    pub fn current_phase(&self) -> TurnPhase {
        self.state
    }

    /// Returns partial content from unflushed buffers.
    ///
    /// Used when the user interrupts streaming and wants to continue with
    /// assistant prefill.
    pub fn peek_partial_content(&self) -> Option<String> {
        self.event_builder.peek_partial_content()
    }

    /// Resets the coordinator state back to Streaming for a new cycle.
    ///
    /// Used after handling a Continue action with prefill - the partial content
    /// has been injected into the thread, and we're ready to receive the
    /// continuation from the LLM.
    pub fn prepare_continuation(&mut self) {
        // Clear any partial buffers since we're starting fresh with prefill
        self.event_builder = EventBuilder::new();
        self.chat_renderer.reset();
        self.structured_renderer.reset();
        self.state = TurnPhase::Streaming;
    }

    /// Flush the renderer's internal markdown buffer to the printer.
    ///
    /// Call this before `Printer::flush_instant()` on interrupt, so any
    /// partial content sitting in the renderer's block buffer gets queued
    /// to the printer and becomes visible before the interrupt menu appears.
    pub fn flush_renderer(&mut self) {
        self.chat_renderer.flush();
    }

    /// Clear the chat renderer's content-kind transition state.
    ///
    /// Delegates to [`ChatResponseRenderer::reset_content_kind`].
    pub fn reset_content_kind(&mut self) {
        self.chat_renderer.reset_content_kind();
    }

    /// Force transition to Complete phase.
    ///
    /// Used when handling hard quit signals (SIGQUIT) where we want to save
    /// progress and exit gracefully without showing an interrupt menu.
    #[cfg(any(unix, test))]
    pub fn force_complete(&mut self) {
        self.state = TurnPhase::Complete;
    }

    /// Handle a hard quit signal during streaming.
    ///
    /// Injects any partial content into the stream and transitions to
    /// Complete so that the turn loop persists and exits.
    #[cfg(any(unix, test))]
    pub fn handle_quit(&mut self, stream: &mut ConversationStream) {
        if let Some(content) = self.peek_partial_content() {
            self.push_event(stream, ChatResponse::message(&content));
        }

        self.force_complete();
    }

    /// Handle an interrupt action during LLM streaming.
    ///
    /// Transitions the state machine based on the user's choice from the
    /// interrupt menu. Content injection (partial content, prefill, replies) is
    /// handled here to keep the state machine self-contained.
    pub fn handle_streaming_interrupt(
        &mut self,
        action: InterruptAction,
        conversation_stream: &mut ConversationStream,
    ) -> TurnPhase {
        match action {
            InterruptAction::Stop => {
                // Inject partial content before completing
                if let Some(content) = self.peek_partial_content() {
                    self.push_event(conversation_stream, ChatResponse::message(&content));
                }

                self.state = TurnPhase::Complete;
            }

            InterruptAction::Abort => self.state = TurnPhase::Aborted,

            InterruptAction::Continue => {
                if let Some(content) = self.peek_partial_content() {
                    self.push_event(conversation_stream, ChatResponse::message(&content));
                }

                self.prepare_continuation();
            }

            InterruptAction::Reply(content) => {
                // Inject partial content as assistant message first
                if let Some(partial) = self.peek_partial_content() {
                    self.push_event(conversation_stream, ChatResponse::message(&partial));
                }

                // Add user's reply as a new request
                self.push_event(conversation_stream, ChatRequest {
                    content,
                    schema: None,
                });
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
    /// - `ToolCancelled`: Cancel tools and continue with cancelled responses
    /// - `RestartTool`: Cancel and restart tool execution
    ///
    /// The actual cancellation is signaled via the `CancellationToken` which
    /// the caller must manage. This method only handles state transitions.
    /// Currently a no-op reserved for future state transitions.
    /// The shell handles cancellation via [`CancellationToken`] and restart
    /// via [`ToolSignalResult`].
    ///
    /// [`CancellationToken`]: tokio_util::sync::CancellationToken
    /// [`ToolSignalResult`]: crate::cmd::query::interrupt::signals::ToolSignalResult
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
