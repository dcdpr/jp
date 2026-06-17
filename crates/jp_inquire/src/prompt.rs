use std::{collections::VecDeque, io::Write};

use inquire::InquireError;
use parking_lot::Mutex;

use crate::{InlineOption, inline_select::InlineSelect};

/// Backend trait for interactive prompts.
///
/// This abstraction enables testing `InterruptHandler` without a real TTY.
///
/// Requires `Send + Sync` to allow use in async contexts and across threads
/// (e.g., `spawn_blocking` for tool prompts).
pub trait PromptBackend: Send + Sync {
    /// Display a single-character inline select menu (like git's `[y/n/q/?]`).
    ///
    /// Returns the selected character, or a default on error.
    fn inline_select(
        &self,
        message: &str,
        options: Vec<InlineOption>,
        default: Option<char>,
        writer: &mut dyn Write,
    ) -> Result<char, InquireError>;

    /// Display a text input prompt.
    ///
    /// Returns the entered text, or empty string on error.
    fn text_input(&self, message: &str, writer: &mut dyn Write) -> Result<String, InquireError>;

    /// Display a single-line text input prompt.
    fn text(
        &self,
        message: &str,
        default: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError>;

    /// Display a selection menu.
    fn select(
        &self,
        message: &str,
        options: Vec<String>,
        default: Option<usize>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError>;
}

/// Blanket impl for references, enabling `&dyn PromptBackend` to work.
impl<P: PromptBackend + ?Sized> PromptBackend for &P {
    fn inline_select(
        &self,
        message: &str,
        options: Vec<InlineOption>,
        default: Option<char>,
        writer: &mut dyn Write,
    ) -> Result<char, InquireError> {
        (*self).inline_select(message, options, default, writer)
    }

    fn text_input(&self, message: &str, writer: &mut dyn Write) -> Result<String, InquireError> {
        (*self).text_input(message, writer)
    }

    fn text(
        &self,
        message: &str,
        default: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        (*self).text(message, default, writer)
    }

    fn select(
        &self,
        message: &str,
        options: Vec<String>,
        default: Option<usize>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        (*self).select(message, options, default, writer)
    }
}

/// Terminal prompt backend using `jp_inquire` and `inquire`.
#[derive(Debug, Default, Clone, Copy)]
pub struct TerminalPromptBackend;

impl PromptBackend for TerminalPromptBackend {
    fn inline_select(
        &self,
        message: &str,
        options: Vec<InlineOption>,
        default: Option<char>,
        writer: &mut dyn Write,
    ) -> Result<char, InquireError> {
        let mut prompt = InlineSelect::new(message, options);
        if let Some(c) = default {
            prompt = prompt.with_default(c);
        }
        prompt.prompt(writer)
    }

    fn text_input(&self, message: &str, writer: &mut dyn Write) -> Result<String, InquireError> {
        inquire::Editor::new(message).prompt_with_writer(writer)
    }

    fn text(
        &self,
        message: &str,
        default: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        let mut prompt = inquire::Text::new(message);
        if let Some(s) = default {
            prompt = prompt.with_default(s);
        }
        prompt.prompt_with_writer(writer)
    }

    fn select(
        &self,
        message: &str,
        options: Vec<String>,
        default: Option<usize>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        let mut prompt = inquire::Select::new(message, options);
        if let Some(idx) = default {
            prompt = prompt.with_starting_cursor(idx);
        }
        prompt.prompt_with_writer(writer)
    }
}

/// Mock prompt backend for testing.
///
/// Pre-load responses that will be returned by `inline_select` and
/// `text_input`.
///
/// Uses `Mutex` instead of `RefCell` to satisfy `Send + Sync` bounds.
#[derive(Debug, Default)]
pub struct MockPromptBackend {
    inline_responses: Mutex<VecDeque<char>>,
    text_responses: Mutex<VecDeque<String>>,
    select_responses: Mutex<VecDeque<String>>,
}

impl MockPromptBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_inline_responses(self, responses: impl IntoIterator<Item = char>) -> Self {
        *self.inline_responses.lock() = responses.into_iter().collect();
        self
    }

    /// Add responses to the inline select menu.
    #[must_use]
    pub fn with_text_responses(
        self,
        responses: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        *self.text_responses.lock() = responses.into_iter().map(Into::into).collect();
        self
    }

    #[must_use]
    pub fn with_select_responses(
        self,
        responses: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        *self.select_responses.lock() = responses.into_iter().map(Into::into).collect();
        self
    }
}

impl PromptBackend for MockPromptBackend {
    fn inline_select(
        &self,
        _message: &str,
        _options: Vec<InlineOption>,
        _default: Option<char>,
        _writer: &mut dyn Write,
    ) -> Result<char, InquireError> {
        self.inline_responses
            .lock()
            .pop_front()
            .ok_or(InquireError::OperationCanceled)
    }

    fn text_input(&self, _message: &str, _writer: &mut dyn Write) -> Result<String, InquireError> {
        self.text_responses
            .lock()
            .pop_front()
            .ok_or(InquireError::OperationCanceled)
    }

    fn text(
        &self,
        _message: &str,
        _default: Option<&str>,
        _writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.text_responses
            .lock()
            .pop_front()
            .ok_or(InquireError::OperationCanceled)
    }

    fn select(
        &self,
        _message: &str,
        _options: Vec<String>,
        _default: Option<usize>,
        _writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.select_responses
            .lock()
            .pop_front()
            .ok_or(InquireError::OperationCanceled)
    }
}
