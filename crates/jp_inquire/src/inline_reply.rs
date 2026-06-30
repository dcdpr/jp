//! Inline reply widget: a rich, multi-line editable prompt for short replies.
//!
//! [`InlineReply`] is built on the vendored `reedline` line editor.
//! It accepts a typed reply inline, supports multi-line input (`Shift+Enter` /
//! `Alt+Enter`), and offers a `Ctrl+X` escape hatch that asks the caller to
//! open the configured external editor.
//! The widget itself never spawns an editor — it only signals intent via
//! [`ReplyOutcome::OpenEditor`], keeping `jp_inquire` free of editor concerns.

use std::{borrow::Cow, io::Write};

use crossterm::cursor::SetCursorStyle;
use inquire::InquireError;
use reedline::{
    CursorConfig, EditCommand, EditMode, Emacs, KeyCode, KeyModifiers, Keybindings, Prompt,
    PromptEditMode, PromptHistorySearch, Reedline, ReedlineEvent, Signal, Vi,
    default_emacs_keybindings, default_vi_insert_keybindings, default_vi_normal_keybindings,
};

/// Host-command sentinel used to surface the editor-escape keybinding through
/// reedline's `ExecuteHostCommand` → [`Signal::HostCommand`] passthrough.
///
/// The leading NUL keeps it from colliding with any value a user could type.
const OPEN_EDITOR_SENTINEL: &str = "\u{0}jp:open-editor";

/// Editing style for the inline reply buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplyEditMode {
    /// Emacs-style keybindings.
    #[default]
    Emacs,

    /// Vi-style modal editing (insert/normal modes).
    Vi,
}

/// The outcome of an [`InlineReply`] prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyOutcome {
    /// The user pressed Enter.
    ///
    /// Carries the buffer, which may be empty — the meaning of an empty
    /// submission is the caller's policy.
    Submit(String),

    /// The user cancelled with `Ctrl+C`.
    Cancelled,

    /// The user asked to escalate to the external editor (`Ctrl+X`).
    ///
    /// Carries the current buffer so the caller can seed the editor with it.
    OpenEditor {
        /// The buffer contents at the time the editor was requested.
        current_text: String,
    },
}

/// A prompt for a short, optionally multi-line reply.
pub struct InlineReply {
    message: String,
    initial_text: String,
    help_message: Option<String>,
    edit_mode: ReplyEditMode,
}

impl InlineReply {
    /// Create a reply prompt with the given message, rendered before the
    /// buffer.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            initial_text: String::new(),
            help_message: None,
            edit_mode: ReplyEditMode::default(),
        }
    }

    /// Seed the buffer with `text` (e.g. the output of a prior editor escape).
    #[must_use]
    pub fn with_initial_text(mut self, text: impl Into<String>) -> Self {
        self.initial_text = text.into();
        self
    }

    /// Set a help message rendered alongside the prompt (e.g. advertising the
    /// `Alt+Enter` newline fallback).
    #[must_use]
    pub fn with_help_message(mut self, msg: impl Into<String>) -> Self {
        self.help_message = Some(msg.into());
        self
    }

    /// Select the editing style of the inline buffer.
    #[must_use]
    pub fn with_edit_mode(mut self, mode: ReplyEditMode) -> Self {
        self.edit_mode = mode;
        self
    }

    /// Display the prompt and block until the user submits, cancels, or
    /// requests the external editor.
    ///
    /// `output` is the stream reedline renders to (the caller's `/dev/tty`
    /// writer); it is owned for the duration of the prompt.
    /// The caller drains any buffered terminal output before calling and
    /// restores it afterwards.
    pub fn prompt(&self, output: Box<dyn Write + Send>) -> Result<ReplyOutcome, InquireError> {
        let mut engine = Reedline::create()
            .with_output(output)
            .with_edit_mode(self.build_edit_mode())
            .use_kitty_keyboard_enhancement(true);

        if self.edit_mode == ReplyEditMode::Vi {
            // Distinct cursor shapes for insert/normal, as a vi user expects.
            engine = engine.with_cursor_config(CursorConfig {
                vi_insert: Some(SetCursorStyle::BlinkingBar),
                vi_normal: Some(SetCursorStyle::SteadyBlock),
                emacs: None,
            });
        }

        if !self.initial_text.is_empty() {
            engine.run_edit_commands(&[EditCommand::InsertString(self.initial_text.clone())]);
        }

        let prompt = ReplyPrompt {
            message: self.message.clone(),
            help: self.help_message.clone().unwrap_or_default(),
        };

        let signal = engine.read_line(&prompt).map_err(InquireError::IO)?;
        Ok(outcome_from_signal(
            signal,
            engine.current_buffer_contents(),
        ))
    }

    /// Build the reedline edit mode, registering JP's custom bindings into the
    /// emacs map, or into both vi keymaps (insert *and* normal) so the editor
    /// escape and newline bindings work regardless of vi mode.
    fn build_edit_mode(&self) -> Box<dyn EditMode> {
        match self.edit_mode {
            ReplyEditMode::Emacs => {
                let mut keybindings = default_emacs_keybindings();
                add_custom_bindings(&mut keybindings);
                Box::new(Emacs::new(keybindings))
            }
            ReplyEditMode::Vi => {
                let mut insert = default_vi_insert_keybindings();
                add_custom_bindings(&mut insert);
                let mut normal = default_vi_normal_keybindings();
                add_custom_bindings(&mut normal);
                Box::new(Vi::new(insert, normal))
            }
        }
    }
}

/// Register JP's custom bindings into `keybindings`:
///
/// - `Ctrl+X` escapes to the external editor (via the host-command sentinel).
/// - `Shift+Enter` (kitty protocol) and `Alt+Enter` (portable fallback) insert
///   a newline for multi-line input.
fn add_custom_bindings(keybindings: &mut Keybindings) {
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('x'),
        ReedlineEvent::ExecuteHostCommand(OPEN_EDITOR_SENTINEL.to_owned()),
    );
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
    keybindings.add_binding(
        KeyModifiers::ALT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
}

/// Map a reedline [`Signal`] to a [`ReplyOutcome`].
///
/// `buffer` is the engine's current buffer, used when the editor escape fires:
/// reedline leaves the buffer intact when `ExecuteHostCommand` exits, so the
/// caller can seed the editor with what was already typed.
fn outcome_from_signal(signal: Signal, buffer: &str) -> ReplyOutcome {
    match signal {
        Signal::HostCommand(cmd) if cmd == OPEN_EDITOR_SENTINEL => ReplyOutcome::OpenEditor {
            current_text: buffer.to_owned(),
        },
        // A normal submission, or a stray external break (no break signal is
        // installed, so this is not expected) — submit what was typed.
        Signal::Success(text) | Signal::ExternalBreak(text) => ReplyOutcome::Submit(text),
        // Ctrl+C / Ctrl+D, any non-sentinel host command, and any future
        // (`#[non_exhaustive]`) variant cancel the reply.
        _ => ReplyOutcome::Cancelled,
    }
}

/// Minimal reedline [`Prompt`] in JP's reply style: the message, the buffer,
/// and an optional help string on the right.
struct ReplyPrompt {
    message: String,
    help: String,
}

impl Prompt for ReplyPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.message)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.help)
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed(" ")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("… ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        _history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        Cow::Borrowed("")
    }
}

#[cfg(test)]
#[path = "inline_reply_tests.rs"]
mod tests;
