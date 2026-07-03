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
//! Choosing to reply (`r` while streaming or while tools run) opens the inline
//! reply widget ([`jp_inquire::InlineReply`]), which renders to the caller's
//! `/dev/tty` writer and offers a `Ctrl+X` escape to the configured external
//! editor.
//! Setting `interrupt.{streaming,tool_call}.compose_in_editor` skips the inline
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
        ComposeInEditor, StreamingInterruptAction, StreamingInterruptConfig, ToolInterruptAction,
        ToolInterruptConfig,
    },
};
use jp_editor::{EditOutcome, EditorBackend, EditorError};
use jp_inquire::{
    InlineOption, ReplyEditMode, ReplyOutcome,
    prompt::{PromptBackend, TerminalPromptBackend},
};
use jp_printer::Printer;

use crate::editor::report_editor_failure;

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

    /// Begin a graceful shutdown.
    ///
    /// Produced when an interrupt menu itself is cancelled with Ctrl-C:
    /// pressing Ctrl-C on the menu escalates past it.
    /// The streaming path commits partial content before completing; the tool
    /// path cancels the running tools.
    Escalate,
}

/// Outcome of collecting a reply from the user.
enum ReplyResult {
    /// The user submitted a non-empty reply.
    Reply(String),

    /// The user submitted an empty (or whitespace-only) reply: "send nothing".
    /// The call site commits forward (the canned tool message, or back to a
    /// streaming menu that has no separate nothing-to-send action).
    Empty,

    /// The user pressed `Ctrl+C` (or the prompt errored): "back up a level".
    /// The call site returns to the interrupt menu where one exists, and
    /// otherwise falls back like `Empty`.
    Cancelled,
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
    /// `compose_in_editor` opt-in.
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
    /// Cancelling the menu itself with `Ctrl+C` escalates: the caller should
    /// commit partial content and begin a graceful shutdown.
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

                    let selected = self.backend.inline_select(
                        "Interrupted",
                        options,
                        None,
                        &mut printer.prompt_writer(),
                    );

                    // A Ctrl-C that cancels the interrupt menu is an
                    // escalation, not a "continue".
                    match selected {
                        Ok(choice) => choice,
                        Err(_) => return InterruptAction::Escalate,
                    }
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
                'r' => match self.collect_reply("Reply:", config.compose_in_editor, printer) {
                    ReplyResult::Reply(text) => return InterruptAction::Reply(text),
                    // Empty submit or `Ctrl+C` in a menu-driven reply re-shows
                    // the menu (the loop iterates).
                    ReplyResult::Empty | ReplyResult::Cancelled if menu => {}
                    // A configured (menu-less) reply has no menu to return to,
                    // so it mirrors the `'c'` branch: keep polling a live
                    // stream, otherwise continue from the partial response.
                    ReplyResult::Empty | ReplyResult::Cancelled if stream_alive => {
                        return InterruptAction::Resume;
                    }
                    ReplyResult::Empty | ReplyResult::Cancelled => {
                        return InterruptAction::Continue;
                    }
                },
                _ => unreachable!("unexpected interrupt choice"),
            }
        }
    }

    /// Handle an interrupt during tool execution.
    ///
    /// Presents a menu with options to stop & respond, restart, or continue
    /// waiting.
    /// Choosing "Stop & respond" collects a response: a typed message stops the
    /// tool and sends it, an empty submission stops with the canned default,
    /// and `Ctrl+C` backs out to the menu.
    /// Cancelling the menu itself with `Ctrl+C` escalates: the caller should
    /// cancel the tools and begin a graceful shutdown.
    ///
    /// When `config.action` is `prompt` the interrupt menu is shown; otherwise
    /// the configured action runs directly without a menu.
    pub fn handle_tool_interrupt(
        &self,
        config: &ToolInterruptConfig,
        printer: &Printer,
    ) -> InterruptAction {
        let menu = config.action == ToolInterruptAction::Prompt;

        loop {
            let choice = match config.action {
                ToolInterruptAction::Prompt => {
                    let options = vec![
                        InlineOption::new('c', "Continue"),
                        InlineOption::new('r', "Stop & respond"),
                        InlineOption::new('t', "Restart"),
                    ];

                    let selected = self.backend.inline_select(
                        "Interrupted",
                        options,
                        None,
                        &mut printer.prompt_writer(),
                    );

                    // A Ctrl-C that cancels the interrupt menu is an
                    // escalation, not a "continue".
                    match selected {
                        Ok(choice) => choice,
                        Err(_) => return InterruptAction::Escalate,
                    }
                }
                ToolInterruptAction::Continue => 'c',
                ToolInterruptAction::Restart => 't',
                ToolInterruptAction::Respond => 'r',
            };

            match choice {
                'c' => return InterruptAction::Resume,
                't' => return InterruptAction::RestartTool,
                'r' => match self.collect_reply("Reply:", config.compose_in_editor, printer) {
                    ReplyResult::Reply(text) => {
                        return InterruptAction::ToolCancelled { response: text };
                    }
                    // `Ctrl+C` backs up to the menu (the loop iterates). A
                    // menu-less configured `respond` has no menu, so it falls
                    // through to the canned message below.
                    ReplyResult::Cancelled if menu => {}
                    // An empty submission stops the tool with the canned "no
                    // explanation" message; so does a menu-less `Ctrl+C`.
                    ReplyResult::Empty | ReplyResult::Cancelled => {
                        return InterruptAction::ToolCancelled {
                            response: DEFAULT_TOOL_CANCELLED_RESPONSE.to_owned(),
                        };
                    }
                },
                _ => unreachable!("unexpected interrupt choice"),
            }
        }
    }

    /// Collect a reply according to the `compose_in_editor` mode.
    ///
    /// - `false` / `"never"`: collect through the inline widget (the `Ctrl+X`
    ///   editor escape is wired only for `false`).
    /// - `true` / `"always"`: open the editor seeded empty.
    ///   A non-empty save is sent; an empty or cancelled editor returns to the
    ///   menu.
    ///   When the editor can't run, `true` falls back to the inline widget and
    ///   `"always"` returns to the menu (never the inline widget).
    fn collect_reply(
        &self,
        message: &str,
        compose: ComposeInEditor,
        printer: &Printer,
    ) -> ReplyResult {
        // Inline-first modes (`false` / `"never"`).
        if !compose.starts_in_editor() {
            return self.collect_reply_inline(message, compose.editor_escape(), printer);
        }

        // Editor-first modes (`true` / `"always"`): open the editor directly.
        let Some(editor) = self.editor.as_ref() else {
            // No editor configured: nothing to open.
            return self.editor_unavailable(message, compose, printer, None);
        };

        match editor.edit_text("") {
            Ok((EditOutcome::Saved, text)) if !text.trim().is_empty() => ReplyResult::Reply(text),
            // Empty save or a cancelled (non-zero-exit) editor: the user bailed,
            // so return to the menu.
            Ok(_) => ReplyResult::Cancelled,
            // The editor could not run.
            Err(error) => self.editor_unavailable(message, compose, printer, Some(error)),
        }
    }

    /// Handle an editor-first mode (`true` / `"always"`) when the editor can't
    /// be used — a spawn/I/O failure or no editor configured.
    ///
    /// `true` falls back to the inline widget so the user can still reply;
    /// `"always"` returns to the menu, never the inline widget (the user opted
    /// out of it).
    /// A spawn failure is surfaced on the chrome channel.
    fn editor_unavailable(
        &self,
        message: &str,
        compose: ComposeInEditor,
        printer: &Printer,
        error: Option<EditorError>,
    ) -> ReplyResult {
        if compose.falls_back_to_inline() {
            if let Some(error) = error {
                report_editor_failure(printer, &error, "Continuing with the inline editor.");
            }
            return self.collect_reply_inline(message, compose.editor_escape(), printer);
        }

        // `"always"`: never the inline widget.
        match error {
            Some(error) => report_editor_failure(printer, &error, "Returning to the menu."),
            None => printer.eprintln("\n⚠ No editor configured; returning to the menu."),
        }
        ReplyResult::Cancelled
    }

    /// Collect a reply through the inline widget.
    ///
    /// The `Ctrl+X` editor escape always returns to the inline prompt — it is
    /// never a terminal action.
    /// The only exits are a submission (empty or not) and `Ctrl+C` (or a prompt
    /// error), handled explicitly, never swallowed.
    fn collect_reply_inline(
        &self,
        message: &str,
        editor_escape: bool,
        printer: &Printer,
    ) -> ReplyResult {
        let mut buffer = String::new();
        loop {
            let output = printer.owned_prompt_writer();
            match self
                .backend
                .inline_reply(message, &buffer, self.edit_mode, editor_escape, output)
            {
                Ok(ReplyOutcome::OpenEditor { current_text }) => {
                    // The editor escape always returns here; whatever was typed
                    // before `Ctrl+X` is preserved.
                    buffer = current_text;
                    if let Some(editor) = self.editor.as_ref() {
                        match editor.edit_text(&buffer) {
                            // Re-seed with the editor's output, even if empty.
                            Ok((EditOutcome::Saved, edited)) => buffer = edited,
                            // Aborted editor: keep the buffer as it was.
                            Ok((EditOutcome::Cancelled, _)) => {}
                            // A spawn / I/O failure is surfaced (chrome +
                            // diagnostics); the buffer is kept.
                            Err(error) => {
                                report_editor_failure(printer, &error, "Keeping your text.");
                            }
                        }
                    }
                }
                Ok(ReplyOutcome::Submit(text)) if !text.trim().is_empty() => {
                    return ReplyResult::Reply(text);
                }
                // A blank (empty or whitespace-only) submission commits forward
                // with nothing; `Ctrl+C` or a prompt error backs up a level.
                // Whitespace counts as blank so the tool path reaches its canned
                // rejection rather than sending a blank-looking reply.
                Ok(ReplyOutcome::Submit(_)) => return ReplyResult::Empty,
                Ok(ReplyOutcome::Cancelled) | Err(_) => return ReplyResult::Cancelled,
            }
        }
    }
}

#[cfg(test)]
#[path = "handler_tests.rs"]
mod tests;
