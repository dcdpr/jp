//! Interrupt handling for the query stream pipeline.
//!
//! When the user presses Ctrl+C during a query, the `InterruptHandler` presents
//! a context-aware menu based on the current state (streaing vs tool
//! execution).
//!
//! The handler returns an [`InterruptAction`] that the caller can use to
//! determine the next step.
//!
//! ## Testing
//!
//! The handler uses dependency injection via [`PromptBackend`] to enable
//! testing without a real TTY.
//! In production, [`TerminalPromptBackend`] uses [`jp_inquire`].
//! In tests, [`MockPromptBackend`] provides pre-programmed responses.
//!
//! [`MockPromptBackend`]: jp_inquire::prompt::MockPromptBackend
//! [`TerminalPromptBackend`]: jp_inquire::prompt::TerminalPromptBackend

use std::io::Write;

use jp_config::interrupt::{
    StreamingInterruptAction, StreamingInterruptConfig, ToolInterruptAction, ToolInterruptConfig,
};
use jp_inquire::{
    InlineOption,
    prompt::{PromptBackend, TerminalPromptBackend},
};

/// Default response sent to the LLM when the user cancels a tool without
/// supplying a custom message.
const DEFAULT_TOOL_CANCELLED_RESPONSE: &str = indoc::concatdoc! {"
    This tool request was intentionally rejected by the user. \
    Please evaluate and either ask the user why it was rejected, \
    or infer the reason by looking at the historical messages \
    in the conversation.\
"};

/// Actions that can be taken after an interrupt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptAction {
    /// Stop generation gracefully.
    Stop,

    /// Abort generation, without saving the current cycle.
    Abort,

    /// Stop generation and immediately reply with a new user message.
    Reply(String),

    /// Resume generation (if stream is alive) or wait (if tool is running).
    Resume,

    /// Continue generation from partial content using assistant prefill.
    ///
    /// When the stream has died (e.g., due to timeout), we can inject the
    /// partial content as an assistant message and ask the LLM to continue from
    /// there.
    Continue,

    /// Cancel all running tools and restart the entire batch.
    RestartTool,

    /// Cancel all running tools and return a user-supplied response to the LLM.
    ///
    /// If the user leaves the response empty, a canned message is used that
    /// instructs the LLM to evaluate why the tool was rejected.
    ToolCancelled { response: String },
}

/// Handles user interrupts (Ctrl+C) during query execution.
///
/// This handler presents interactive menus and returns the user's chosen
/// action.
/// The actual handling of the action is done by the caller.
///
/// Uses [`PromptBackend`] for dependency injection, enabling testing without a
/// TTY.
pub struct InterruptHandler<P: PromptBackend = TerminalPromptBackend> {
    backend: P,
}

impl Default for InterruptHandler<TerminalPromptBackend> {
    fn default() -> Self {
        Self::new()
    }
}

impl InterruptHandler<TerminalPromptBackend> {
    /// Create a new interrupt handler.
    pub fn new() -> Self {
        Self {
            backend: TerminalPromptBackend,
        }
    }
}

impl<P: PromptBackend> InterruptHandler<P> {
    /// Create an interrupt handler with a custom prompt backend.
    pub fn with_backend(backend: P) -> Self {
        Self { backend }
    }

    /// Handle an interrupt during LLM streaming.
    ///
    /// When `config.action` is `prompt` the interrupt menu is shown; otherwise
    /// the configured action runs directly without a menu.
    /// A `reply` still prompts for the reply text.
    pub fn handle_streaming_interrupt(
        &self,
        config: &StreamingInterruptConfig,
        writer: &mut dyn Write,
        stream_alive: bool,
    ) -> InterruptAction {
        let choice = match config.action {
            StreamingInterruptAction::Prompt => {
                let options = vec![
                    InlineOption::new('c', "Continue"),
                    InlineOption::new('r', "Reply (stop & respond)"),
                    InlineOption::new('s', "Stop (save & exit)"),
                    InlineOption::new('a', "Abort (discard & exit)"),
                ];

                self.backend
                    .inline_select("Interrupted", options, None, writer)
                    .unwrap_or('s')
            }
            StreamingInterruptAction::Continue => 'c',
            StreamingInterruptAction::Reply => 'r',
            StreamingInterruptAction::Stop => 's',
            StreamingInterruptAction::Abort => 'a',
        };

        match choice {
            'c' if stream_alive => InterruptAction::Resume,
            'c' => InterruptAction::Continue,
            'r' => InterruptAction::Reply(
                self.backend
                    .text_input("Reply:", writer)
                    .unwrap_or_default(),
            ),
            's' => InterruptAction::Stop,
            'a' => InterruptAction::Abort,
            _ => unreachable!("unexpected choice"),
        }
    }

    /// Handle an interrupt during tool execution.
    ///
    /// Presents a menu with options to stop & reply, restart, or continue
    /// waiting.
    /// When the user chooses "Stop & Reply", they can supply a custom message.
    /// An empty input produces a canned default.
    ///
    /// When `config.action` is `prompt` the interrupt menu is shown; otherwise
    /// the configured action runs directly without a menu.
    /// A `stop_reply` still prompts for the reply text.
    pub fn handle_tool_interrupt(
        &self,
        config: &ToolInterruptConfig,
        writer: &mut dyn Write,
    ) -> InterruptAction {
        let choice = match config.action {
            ToolInterruptAction::Prompt => {
                let options = vec![
                    InlineOption::new('c', "Continue"),
                    InlineOption::new('s', "Stop & Reply"),
                    InlineOption::new('r', "Restart"),
                ];

                self.backend
                    .inline_select("Interrupted", options, None, writer)
                    .unwrap_or('c')
            }
            ToolInterruptAction::Continue => 'c',
            ToolInterruptAction::Restart => 'r',
            ToolInterruptAction::StopReply => 's',
        };

        match choice {
            'c' => InterruptAction::Resume,
            's' => {
                let response = self
                    .backend
                    .text_input("Reply:", writer)
                    .unwrap_or_default();

                let response = if response.trim().is_empty() {
                    DEFAULT_TOOL_CANCELLED_RESPONSE.to_owned()
                } else {
                    response
                };

                InterruptAction::ToolCancelled { response }
            }
            'r' => InterruptAction::RestartTool,
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
