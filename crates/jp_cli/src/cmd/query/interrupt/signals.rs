//! Signal and event handlers for the query stream pipeline.
//!
//! These functions extract the logic from the `jp_macro::select!` closures to
//! improve readability and testability.
//! Each handler:
//!
//! 1. Shows appropriate UI (interrupt menus) when needed
//! 2. Delegates state transitions to the `TurnCoordinator` state machine
//! 3. Returns a `LoopAction` for the caller to handle control flow

use std::sync::Arc;

use jp_config::interrupt::{StreamingInterruptConfig, ToolInterruptConfig};
use jp_conversation::ConversationStream;
use jp_editor::EditorBackend;
use jp_inquire::{ReplyEditMode, prompt::PromptBackend};
use jp_llm::event::{Event, FinishReason, record_patches};
use jp_printer::Printer;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, trace};

use super::handler::{InterruptAction, InterruptHandler};
use crate::cmd::query::{
    stream::{RebuildRefusal, StreamRetryState},
    turn::{Action, CommittedEvent, TurnCoordinator, TurnPhase},
};

/// Action to take in a select loop.
///
/// Used by handlers that operate within a loop context (LLM events).
#[derive(Debug)]
pub enum LoopAction {
    /// Continue the loop (wait for next event).
    Continue,

    /// Break the inner loop.
    Break,

    /// The provider asked to rebuild the request and was refused.
    /// The turn cannot make progress and must abort.
    RebuildRefused(RebuildRefusal),
}

/// Result of handling an interrupt during LLM streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamingInterruptResult {
    /// Keep polling the current stream.
    Continue,

    /// Break the inner streaming loop; the turn phase decides what happens
    /// next.
    Break,

    /// Abort the turn without persisting the current cycle.
    Abort,

    /// The menu itself was cancelled with Ctrl-C: partial content is committed
    /// and the turn is complete.
    /// The caller should begin a graceful shutdown.
    Escalate,

    /// The menu could not be shown, so nothing was decided.
    /// The caller leaves the stream as it was and leaves the press on the
    /// signal router's escalation ladder.
    PromptFailed,
}

/// Handle a Ctrl-C interrupt notification received during LLM streaming.
///
/// Applies the configured streaming interrupt behavior: the menu is shown only
/// when `config.action` is `prompt`, otherwise the configured action runs
/// directly.
/// Then delegates to the turn coordinator's state machine for state transitions
/// and content injection.
pub fn handle_streaming_interrupt(
    turn_coordinator: &mut TurnCoordinator,
    conversation_stream: &mut ConversationStream,
    printer: &Printer,
    backend: &dyn PromptBackend,
    editor: Option<Arc<dyn EditorBackend>>,
    edit_mode: ReplyEditMode,
    config: &StreamingInterruptConfig,
    llm_stream_finished: bool,
) -> StreamingInterruptResult {
    let action = decide_streaming_interrupt(
        turn_coordinator,
        printer,
        backend,
        editor,
        edit_mode,
        config,
        llm_stream_finished,
    );

    apply_streaming_interrupt(action, turn_coordinator, conversation_stream)
}

/// Ask the user what an interrupt during LLM streaming should do.
///
/// Shows the menu when `config.action` is `prompt`, and otherwise resolves the
/// configured action without asking.
/// Decides only: nothing is committed to the conversation and the turn's phase
/// does not move until the action reaches [`apply_streaming_interrupt`].
///
/// `llm_stream_finished` narrows the menu to what a dead stream can still
/// offer.
pub fn decide_streaming_interrupt(
    turn_coordinator: &mut TurnCoordinator,
    printer: &Printer,
    backend: &dyn PromptBackend,
    editor: Option<Arc<dyn EditorBackend>>,
    edit_mode: ReplyEditMode,
    config: &StreamingInterruptConfig,
    llm_stream_finished: bool,
) -> InterruptAction {
    info!("Interrupt received during streaming.");

    // Flush the renderer's markdown buffer to the printer queue, then drain
    // the printer queue instantly (skip typewriter delays) so all generated
    // content is visible before the interrupt menu appears.
    turn_coordinator.flush_renderer();
    printer.flush_instant();

    let action = InterruptHandler::with_backend(backend, editor, edit_mode)
        .handle_streaming_interrupt(config, printer, !llm_stream_finished);

    debug!(
        ?action,
        llm_stream_finished, "Streaming interrupt resolved."
    );

    action
}

/// Apply an already-decided interrupt action to a streaming turn.
///
/// Commits partial assistant content, appends a reply, or moves the phase,
/// depending on the action, and reports what the streaming loop should do next.
///
/// The action's source does not matter here: a menu choice and a command that
/// arrived from a client are applied identically.
pub fn apply_streaming_interrupt(
    action: InterruptAction,
    turn_coordinator: &mut TurnCoordinator,
    conversation_stream: &mut ConversationStream,
) -> StreamingInterruptResult {
    // `Resume` means "keep waiting for the current stream." The state
    // machine is a no-op for it, and we must NOT break the inner loop:
    // breaking drops the live `SelectAll` and forces a redundant new
    // HTTP request, which can land us in inconsistent state. Continue
    // polling instead.
    let is_resume = matches!(action, InterruptAction::Resume);
    let is_escalate = matches!(action, InterruptAction::Escalate);

    // A menu that never ran decided nothing, so the state machine is left
    // untouched: no partial commit, no phase change.
    if matches!(action, InterruptAction::PromptFailed) {
        return StreamingInterruptResult::PromptFailed;
    }

    // Delegate state transition to the turn coordinator
    let result = match turn_coordinator.handle_streaming_interrupt(action, conversation_stream) {
        // Return without persisting this cycle (previous turn cycles
        // are already persisted).
        TurnPhase::Aborted => StreamingInterruptResult::Abort,

        // Partial content is committed and the phase is Complete; the
        // caller begins the graceful shutdown.
        _ if is_escalate => StreamingInterruptResult::Escalate,

        // Resume keeps the existing stream alive.
        _ if is_resume => StreamingInterruptResult::Continue,

        // All other phases break from loop, persist, then outer loop
        // decides.
        _ => StreamingInterruptResult::Break,
    };

    debug!(?result, phase = ?turn_coordinator.current_phase(), "Streaming interrupt handled.");
    result
}

/// Handle a successful event from the LLM stream.
///
/// Stream errors are handled separately by [`handle_stream_error`], which is
/// the single source of truth for all retry logic.
///
/// Returns the loop-control signal alongside any committed event the shell
/// should react to immediately.
/// The committed event is surfaced directly from [`EventBuilder::handle_flush`]
/// (via the coordinator) so the shell never has to infer it from the
/// conversation stream's tail — a duplicate flush from a misbehaving provider
/// commits nothing and so cannot cause a double dispatch.
///
/// [`EventBuilder::handle_flush`]: jp_llm::event_builder::EventBuilder::handle_flush
/// [`handle_stream_error`]: crate::cmd::query::stream::handle_stream_error
pub fn handle_llm_event(
    event: Event,
    turn_coordinator: &mut TurnCoordinator,
    conversation_stream: &mut ConversationStream,
    retry_state: &mut StreamRetryState,
) -> (LoopAction, CommittedEvent) {
    // `Patch` is a side-channel instruction from the provider to fix bad events
    // in the conversation. This can be handled directly instead of passing
    // through the turn coordinator.
    if let Event::Patch(patches) = event {
        let count = record_patches(conversation_stream, &patches);
        let shrinks = patches
            .iter()
            .all(|patch| patch.action.shrinks_projection());
        retry_state.record_patch(count, shrinks);

        if !shrinks {
            // A patch set that can grow the projection gives no guarantee the
            // repair loop ends, so the rebuild below is refused whatever it
            // changed.
            tracing::warn!(
                patches = patches.len(),
                "History patches include an action that may not shrink the projected conversation."
            );
        }

        if count > 0 {
            tracing::debug!(count, "Recorded history patch overlay.");
        } else {
            // The rebuilt request would be byte-identical, so the rebuild that
            // follows is refused below rather than resent.
            tracing::warn!(
                patches = patches.len(),
                "History patches change no events in the projected conversation."
            );
        }

        return (LoopAction::Continue, CommittedEvent::None);
    }

    // `Retry` means the provider wants us to rebuild the request and try again.
    // Break the inner streaming loop while keeping the phase as `Streaming` so
    // the outer turn loop re-enters with a fresh request.
    if matches!(event, Event::Finished(FinishReason::Retry)) {
        return match retry_state.authorize_rebuild() {
            Ok(()) => (LoopAction::Break, CommittedEvent::None),
            Err(refusal) => (LoopAction::RebuildRefused(refusal), CommittedEvent::None),
        };
    }

    let outcome = turn_coordinator.handle_event(conversation_stream, event);
    let loop_action = match outcome.action {
        Action::Done | Action::ExecuteTools => LoopAction::Break,
        Action::Continue | Action::SendFollowUp => LoopAction::Continue,
    };

    (loop_action, outcome.committed)
}

/// Result of handling an interrupt during tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolInterruptResult {
    /// Continue waiting for tool execution to complete.
    Continue,

    /// A tool prompt is active; the interrupt was not handled here.
    /// The caller should pass it down the handler stack.
    Declined,

    /// Cancel current execution and restart with the same tools.
    /// The caller should wait for cancellation to complete, then re-execute.
    Restart,

    /// Cancel current execution and override cancelled tool responses.
    Cancelled {
        /// The user-supplied message, or `None` to answer each cancelled tool
        /// with its configured `cancellation_response`.
        response: Option<String>,

        /// Whether to end the turn after recording the cancelled responses,
        /// instead of sending them back to the assistant in a follow-up
        /// request.
        exit: bool,
    },

    /// Cancel current execution and begin a graceful shutdown: the user
    /// cancelled the interrupt menu itself with Ctrl-C.
    Escalate,

    /// The menu could not be shown, so nothing was decided.
    /// The caller keeps waiting for the running tools and leaves the press on
    /// the signal router's escalation ladder.
    PromptFailed,
}

/// What the tool interrupt menu needs in order to run.
///
/// The menu is the only part of an execution phase that talks to the terminal
/// on its own, so its dependencies travel together and reach nothing else.
pub struct InterruptUi<'a> {
    /// Records the state transition the chosen action implies.
    pub turn_coordinator: &'a mut TurnCoordinator,

    /// Where the menu draws.
    pub printer: &'a Printer,

    /// Reads the user's choice; a test supplies a scripted one.
    pub backend: &'a dyn PromptBackend,

    /// Opens the editor a "Stop & respond" reply may use.
    pub editor: Option<Arc<dyn EditorBackend>>,

    /// Which editing style that reply uses.
    pub edit_mode: ReplyEditMode,
}

/// Handle a Ctrl-C interrupt notification received during tool execution.
///
/// Applies the configured tool interrupt behavior: the menu is shown only when
/// `config.action` is `prompt`, otherwise the configured action runs directly.
/// Then delegates to the turn coordinator for state machine updates.
///
/// If any tool is currently showing an interactive prompt (permission,
/// question, result edit), the interrupt is declined: the active prompt handles
/// Ctrl+C itself, and the caller should pass the notification down the handler
/// stack.
pub fn handle_tool_interrupt(
    cancellation_token: &CancellationToken,
    is_prompting: bool,
    ui: &mut InterruptUi<'_>,
    config: &ToolInterruptConfig,
) -> ToolInterruptResult {
    if is_prompting {
        trace!("Declining interrupt: tool prompt is active");
        return ToolInterruptResult::Declined;
    }

    let action = InterruptHandler::with_backend(ui.backend, ui.editor.clone(), ui.edit_mode)
        .handle_tool_interrupt(config, ui.printer);
    debug!(?action, "Tool interrupt resolved.");

    apply_tool_interrupt(action, cancellation_token, ui.turn_coordinator)
}

/// Restate an interrupt decided elsewhere in the terms tool execution acts on.
///
/// The two menus offer different verbs for the same intents, and only these
/// ones mean anything while tools are running: a reply becomes the answer each
/// cancelled tool gives back, which is what `[r] Stop & respond` does at the
/// terminal, and stopping cancels them and ends the turn.
#[must_use]
pub fn as_tool_interrupt(action: InterruptAction) -> InterruptAction {
    match action {
        InterruptAction::Reply { content, .. } => InterruptAction::ToolCancelled {
            response: Some(content),
            exit: false,
        },

        // Abort discards the turn's uncommitted work, and cancelling the
        // running tools is as much of that as this phase owns.
        InterruptAction::Stop | InterruptAction::Abort => InterruptAction::ToolCancelled {
            response: None,
            exit: true,
        },

        other => other,
    }
}

/// Apply an already-decided interrupt action to running tools.
///
/// Cancels the running tools where the action calls for it, and reports what
/// the execution loop should do with the results.
///
/// The action's source does not matter here: a menu choice and a command that
/// arrived from a client are applied identically.
pub fn apply_tool_interrupt(
    action: InterruptAction,
    cancellation_token: &CancellationToken,
    turn_coordinator: &mut TurnCoordinator,
) -> ToolInterruptResult {
    // A menu that never ran decided nothing: the running tools are left alone
    // and the state machine is not notified.
    if matches!(action, InterruptAction::PromptFailed) {
        return ToolInterruptResult::PromptFailed;
    }

    // Notify the state machine (reserved for future state transitions).
    turn_coordinator.handle_tool_interrupt(&action);

    let result = match action {
        InterruptAction::RestartTool => {
            info!("Restarting tool execution");
            ToolInterruptResult::Restart
        }
        InterruptAction::ToolCancelled { response, exit } => {
            ToolInterruptResult::Cancelled { response, exit }
        }
        InterruptAction::Escalate => {
            info!("Escalating past the tool interrupt menu");
            cancellation_token.cancel();
            ToolInterruptResult::Escalate
        }
        _ => ToolInterruptResult::Continue,
    };

    debug!(?result, "Tool interrupt handled.");
    result
}

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;
