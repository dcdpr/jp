//! Signal and event handlers for the query stream pipeline.
//!
//! These functions extract the logic from the `jp_macro::select!` closures
//! to improve readability and testability. Each handler:
//! 1. Shows appropriate UI (interrupt menus) when needed
//! 2. Delegates state transitions to the `TurnCoordinator` state machine
//! 3. Returns a `LoopAction` for the caller to handle control flow

use jp_conversation::ConversationStream;
use jp_inquire::prompt::PromptBackend;
use jp_llm::event::Event;
use jp_printer::Printer;
use tokio_util::sync::CancellationToken;
use tracing::{info, trace};

use super::handler::{InterruptAction, InterruptHandler};
use crate::{
    cmd::query::turn::{Action, TurnCoordinator, TurnPhase},
    signals::SignalTo,
};

/// Action to take in a select loop.
///
/// Used by handlers that operate within a loop context (streaming, LLM events).
/// The type parameter `T` is the return type when exiting the function early.
#[derive(Debug)]
pub enum LoopAction<T> {
    /// Continue the loop (wait for next event).
    Continue,

    /// Break the inner loop.
    Break,

    /// Return from the function with the given value.
    Return(T),
}

/// Handle a signal received during LLM streaming.
///
/// Shows the interrupt menu when Ctrl+C is received, then delegates to the
/// turn coordinator's state machine for state transitions and content injection.
pub fn handle_streaming_signal(
    signal: SignalTo,
    turn_coordinator: &mut TurnCoordinator,
    conversation_stream: &mut ConversationStream,
    printer: &Printer,
    backend: &dyn PromptBackend,
    llm_stream_finished: bool,
) -> LoopAction<()> {
    info!(?signal, "Received signal during streaming.");

    match signal {
        #[cfg(any(unix, test))]
        SignalTo::Quit => {
            // Treat Quit like Stop: save partial content and exit gracefully.
            // This ensures we don't lose progress on hard quit signals.
            turn_coordinator.handle_quit(conversation_stream);

            LoopAction::Break
        }

        SignalTo::Shutdown => {
            // Flush the renderer's markdown buffer to the printer queue,
            // then drain the printer queue instantly (skip typewriter
            // delays) so all generated content is visible before the
            // interrupt menu appears.
            turn_coordinator.flush_renderer();
            printer.flush_instant();

            let action = InterruptHandler::with_backend(backend)
                .handle_streaming_interrupt(&mut printer.out_writer(), !llm_stream_finished);

            // Delegate state transition to the turn coordinator
            match turn_coordinator.handle_streaming_interrupt(action, conversation_stream) {
                // Return without persisting this cycle (previous turn cycles
                // are already persisted).
                TurnPhase::Aborted => LoopAction::Return(()),

                // All other phases break from loop, persist, then outer loop
                // decides.
                _ => LoopAction::Break,
            }
        }

        #[cfg(any(unix, test))]
        SignalTo::ReloadFromDisk => LoopAction::Continue,
    }
}

/// Handle a successful event from the LLM stream.
///
/// Stream errors are handled separately by
/// [`handle_stream_error`](crate::cmd::query::stream::handle_stream_error), which
/// is the single source of truth for all retry logic.
pub fn handle_llm_event(
    event: Event,
    turn_coordinator: &mut TurnCoordinator,
    conversation_stream: &mut ConversationStream,
) -> LoopAction<()> {
    let action = turn_coordinator.handle_event(conversation_stream, event);
    match action {
        Action::Done | Action::ExecuteTools(_) => LoopAction::Break,
        _ => LoopAction::Continue,
    }
}

/// Result of handling a signal during tool execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSignalResult {
    /// Continue waiting for tool execution to complete.
    Continue,

    /// Cancel current execution and restart with the same tools.
    /// The caller should wait for cancellation to complete, then re-execute.
    Restart,

    /// Cancel current execution and override cancelled tool responses with
    /// the user-supplied message.
    Cancelled { response: String },
}

/// Handle a signal received during tool execution.
///
/// Shows the tool interrupt menu when Ctrl+C is received, then delegates to
/// the turn coordinator for state machine updates.
///
/// If any tool is currently showing an interactive prompt (permission, question,
/// result edit), the interrupt menu is suppressed and we let the active prompt
/// handle Ctrl+C instead. This prevents UI conflicts between the interrupt menu
/// and the active prompt.
///
/// # Arguments
///
/// * `is_prompting` - Whether any tool is currently showing an interactive prompt.
/// * `backend` - Allows injecting a mock prompt backend for testing.
pub fn handle_tool_signal(
    signal: SignalTo,
    cancellation_token: &CancellationToken,
    turn_coordinator: &mut TurnCoordinator,
    is_prompting: bool,
    printer: &Printer,
    backend: &dyn PromptBackend,
) -> ToolSignalResult {
    match signal {
        #[cfg(any(unix, test))]
        SignalTo::Quit => {
            // For hard quit during tool execution, we cancel and let the normal
            // flow handle persistence (responses will be cancelled).
            cancellation_token.cancel();
            turn_coordinator.force_complete();
            ToolSignalResult::Continue
        }

        SignalTo::Shutdown => {
            // If any tool is showing an interactive prompt, don't show the
            // interrupt menu. Let the active prompt handle Ctrl+C (it will
            // typically return OperationCanceled which the executor handles).
            if is_prompting {
                trace!("Suppressing interrupt menu: tool prompt is active");
                return ToolSignalResult::Continue;
            }

            let action = InterruptHandler::with_backend(backend)
                .handle_tool_interrupt(&mut printer.out_writer());

            // Notify the state machine (reserved for future state transitions).
            turn_coordinator.handle_tool_interrupt(&action);

            match action {
                InterruptAction::RestartTool => {
                    info!("Restarting tool execution");
                    cancellation_token.cancel();
                    ToolSignalResult::Restart
                }
                InterruptAction::ToolCancelled { response } => {
                    cancellation_token.cancel();
                    ToolSignalResult::Cancelled { response }
                }
                _ => ToolSignalResult::Continue,
            }
        }

        #[cfg(any(unix, test))]
        SignalTo::ReloadFromDisk => ToolSignalResult::Continue,
    }
}

#[cfg(test)]
#[path = "signals_tests.rs"]
mod tests;
