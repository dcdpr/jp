//! Interrupt handling for the query stream pipeline.
//!
//! When the user presses Ctrl+C during a query, the `InterruptHandler` presents
//! a context-aware menu based on the current state (streaming vs tool
//! execution).
//!
//! The handler returns an [`InterruptAction`] that the caller can use to
//! determine the next step.
//!
//! ## Replies
//!
//! Choosing to reply (`r` while streaming, `s` while tools run) opens the
//! inline reply widget ([`jp_inquire::InlineReply`]), which renders to the
//! caller's `/dev/tty` writer and offers a `Ctrl+X` escape to the configured
//! external editor.
//! Setting `interrupt.{streaming,tool_call}.reply_in_editor` skips the inline
//! step and opens the editor directly.
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

use std::sync::Arc;

use jp_config::{
    editor::InlineEditMode,
    interrupt::{
        StreamingInterruptAction, StreamingInterruptConfig, ToolInterruptAction,
        ToolInterruptConfig,
    },
};
use jp_editor::{EditOutcome, EditorBackend};
use jp_inquire::{
    InlineOption, ReplyEditMode, ReplyOutcome,
    prompt::{PromptBackend, TerminalPromptBackend},
};
use jp_printer::Printer;

/// Default response sent to the LLM when the user cancels a tool without
/// supplying a custom message.
const DEFAULT_TOOL_CANCELLED_RESPONSE: &str = indoc::concatdoc! {"
    This tool request was intentionally rejected by the user. \
    Please evaluate and either ask the user why it was rejected, \
    or infer the reason by looking at the historical messages \
    in the conversation.\
"};

/// Map the configured inline edit mode onto the reply widget's edit mode.
pub(crate) fn reply_edit_mode(mode: InlineEditMode) -> ReplyEditMode {
    match mode {
        InlineEditMode::Emacs => ReplyEditMode::Emacs,
        InlineEditMode::Vi => ReplyEditMode::Vi,
    }
}

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

/// Outcome of collecting a reply from the user.
enum ReplyResult {
    /// The user submitted a non-empty reply.
    Reply(String),

    /// The user backed out: an empty submission, a cancel, or an emptied or
    /// aborted editor.
    /// The call site decides what backing out means (return to the menu, use a
    /// canned message, …).
    Back,
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
    /// Backend that renders the menu and the inline reply prompt.
    backend: P,

    /// The configured editor, used for the `Ctrl+X` escape and the
    /// `reply_in_editor` opt-in.
    /// `None` when no editor is configured; the inline widget still works.
    editor: Option<Arc<dyn EditorBackend>>,

    /// The inline reply buffer's editing style.
    edit_mode: ReplyEditMode,
}

impl Default for InterruptHandler<TerminalPromptBackend> {
    fn default() -> Self {
        Self::with_backend(TerminalPromptBackend, None, ReplyEditMode::Emacs)
    }
}

impl<P: PromptBackend> InterruptHandler<P> {
    /// Create an interrupt handler with a custom prompt backend, an optional
    /// editor (for the reply escape hatch), and the inline edit mode.
    pub fn with_backend(
        backend: P,
        editor: Option<Arc<dyn EditorBackend>>,
        edit_mode: ReplyEditMode,
    ) -> Self {
        Self {
            backend,
            editor,
            edit_mode,
        }
    }

    /// Handle an interrupt during LLM streaming.
    ///
    /// When `config.action` is `prompt` the interrupt menu is shown; otherwise
    /// the configured action runs directly without a menu.
    /// Choosing `reply` collects a reply; backing out of a menu-driven reply
    /// returns to the menu, while a configured (menu-less) `reply` resumes.
    pub fn handle_streaming_interrupt(
        &self,
        config: &StreamingInterruptConfig,
        printer: &Printer,
        stream_alive: bool,
    ) -> InterruptAction {
        let menu = config.action == StreamingInterruptAction::Prompt;

        loop {
            let choice = match config.action {
                StreamingInterruptAction::Prompt => {
                    let options = vec![
                        InlineOption::new('c', "Continue"),
                        InlineOption::new('r', "Reply (stop & respond)"),
                        InlineOption::new('s', "Stop (save & exit)"),
                        InlineOption::new('a', "Abort (discard & exit)"),
                    ];

                    // A cancelled menu falls back to a graceful stop. (RFD 045's
                    // `Escalated` outcome is not yet implemented; this is the
                    // graceful-shutdown stand-in.)
                    self.backend
                        .inline_select("Interrupted", options, None, &mut printer.prompt_writer())
                        .unwrap_or('s')
                }
                StreamingInterruptAction::Continue => 'c',
                StreamingInterruptAction::Reply => 'r',
                StreamingInterruptAction::Stop => 's',
                StreamingInterruptAction::Abort => 'a',
            };

            match choice {
                'c' if stream_alive => return InterruptAction::Resume,
                'c' => return InterruptAction::Continue,
                's' => return InterruptAction::Stop,
                'a' => return InterruptAction::Abort,
                'r' => match self.collect_reply("Reply:", config.reply_in_editor, printer) {
                    ReplyResult::Reply(text) => return InterruptAction::Reply(text),
                    // Backing out of a menu-driven reply re-shows the menu (the
                    // loop iterates).
                    ReplyResult::Back if menu => {}
                    // A configured (menu-less) reply has no menu to return to,
                    // so it mirrors the `'c'` branch: keep polling a live
                    // stream, otherwise continue from the partial response.
                    ReplyResult::Back if stream_alive => return InterruptAction::Resume,
                    ReplyResult::Back => return InterruptAction::Continue,
                },
                _ => unreachable!("unexpected interrupt choice"),
            }
        }
    }

    /// Handle an interrupt during tool execution.
    ///
    /// Presents a menu with options to stop & reply, restart, or continue
    /// waiting.
    /// When the user chooses "Stop & Reply", they can supply a custom message;
    /// an empty or cancelled reply produces the canned default.
    ///
    /// When `config.action` is `prompt` the interrupt menu is shown; otherwise
    /// the configured action runs directly without a menu.
    pub fn handle_tool_interrupt(
        &self,
        config: &ToolInterruptConfig,
        printer: &Printer,
    ) -> InterruptAction {
        let choice = match config.action {
            ToolInterruptAction::Prompt => {
                let options = vec![
                    InlineOption::new('c', "Continue"),
                    InlineOption::new('s', "Stop & Reply"),
                    InlineOption::new('r', "Restart"),
                ];

                self.backend
                    .inline_select("Interrupted", options, None, &mut printer.prompt_writer())
                    .unwrap_or('c')
            }
            ToolInterruptAction::Continue => 'c',
            ToolInterruptAction::Restart => 'r',
            ToolInterruptAction::StopReply => 's',
        };

        match choice {
            'c' => InterruptAction::Resume,
            'r' => InterruptAction::RestartTool,
            's' => {
                // An empty or cancelled reply falls through to the canned
                // message, preserving the "interrupt a tool with no
                // explanation" shortcut.
                let response = match self.collect_reply("Reply:", config.reply_in_editor, printer) {
                    ReplyResult::Reply(text) => text,
                    ReplyResult::Back => DEFAULT_TOOL_CANCELLED_RESPONSE.to_owned(),
                };

                InterruptAction::ToolCancelled { response }
            }
            _ => unreachable!("unexpected interrupt choice"),
        }
    }

    /// Collect a reply, honoring the straight-to-editor opt-in.
    ///
    /// With `reply_in_editor` set and an editor configured, opens the editor
    /// seeded empty; otherwise collects through the inline widget.
    fn collect_reply(
        &self,
        message: &str,
        reply_in_editor: bool,
        printer: &Printer,
    ) -> ReplyResult {
        if reply_in_editor {
            let Some(editor) = self.editor.as_ref() else {
                // No editor configured: fall back to the inline widget rather
                // than silently doing nothing.
                return self.collect_reply_inline(message, printer);
            };

            return match editor.edit_text("") {
                Ok((EditOutcome::Saved, text)) if !text.trim().is_empty() => {
                    ReplyResult::Reply(text)
                }
                // Empty, cancelled, or a spawn failure: back out.
                _ => ReplyResult::Back,
            };
        }

        self.collect_reply_inline(message, printer)
    }

    /// Collect a reply through the inline widget, looping on the editor escape.
    ///
    /// Prompt errors and `Ctrl+C` are handled explicitly (never swallowed): a
    /// non-`Submit` outcome or an error backs out.
    fn collect_reply_inline(&self, message: &str, printer: &Printer) -> ReplyResult {
        let mut buffer = String::new();
        loop {
            let output = printer.owned_prompt_writer();
            match self
                .backend
                .inline_reply(message, &buffer, self.edit_mode, output)
            {
                Ok(ReplyOutcome::OpenEditor { current_text }) => {
                    let Some(editor) = self.editor.as_ref() else {
                        // No editor configured: the escape is a no-op, the
                        // widget stays open with the buffer intact.
                        continue;
                    };

                    match editor.edit_text(&current_text) {
                        Ok((EditOutcome::Saved, edited)) if !edited.trim().is_empty() => {
                            // Re-seed the inline prompt with the editor's output.
                            buffer = edited;
                        }
                        // Emptied, cancelled, or failed: back out.
                        _ => return ReplyResult::Back,
                    }
                }
                Ok(ReplyOutcome::Submit(text)) if !text.trim().is_empty() => {
                    return ReplyResult::Reply(text);
                }
                // A blank (empty or whitespace-only) submission, a `Ctrl+C`
                // cancel, or a prompt error all back out. Whitespace is treated
                // as blank so the tool path falls through to its canned
                // rejection rather than sending a blank-looking reply.
                Ok(ReplyOutcome::Submit(_) | ReplyOutcome::Cancelled) | Err(_) => {
                    return ReplyResult::Back;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
