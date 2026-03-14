//! Git-style inline select prompt implementation.

use std::{
    fmt::{self, Write as _},
    io,
};

use inquire::{CustomType, InquireError, ui::RenderConfig};

/// Represents a single option in an inline select prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineOption {
    /// The character key for this option (e.g., 'y', 'n', 'q')
    pub key: char,

    /// Human-readable description of what this option does
    pub description: String,
}

impl InlineOption {
    /// Creates a new inline option with the given key and description.
    pub fn new(key: char, description: impl Into<String>) -> Self {
        Self {
            key,
            description: description.into(),
        }
    }
}

/// Git-style inline select prompt.
///
/// This provides a compact, single-line prompt where users select an option by
/// typing a single character. The '?' key always shows help text.
///
/// # Example
///
/// ```no_run
/// use jp_inquire::{InlineOption, InlineSelect};
///
/// let options = vec![
///     InlineOption::new('y', "proceed with the action"),
///     InlineOption::new('n', "skip this action"),
///     InlineOption::new('q', "exit without completing remaining actions"),
/// ];
///
/// let mut buf = Vec::new();
/// let handler = InlineSelect::new("Continue with this operation?", options);
/// match handler.prompt(&mut buf) {
///     Ok(ch) => println!("Selected: {ch}"),
///     Err(error) => eprintln!("Error: {error}"),
/// }
/// ```
pub struct InlineSelect {
    message: String,
    options: Vec<InlineOption>,
    default: Option<char>,
}

impl InlineSelect {
    /// Creates a new inline select prompt with the given message and options.
    ///
    /// The message will be displayed before the option list, like:
    /// `{message} [y,n,q,?]?`
    ///
    /// The '?' option is automatically added to show help.
    pub fn new(message: impl Into<String>, options: Vec<InlineOption>) -> Self {
        Self {
            message: message.into(),
            options,
            default: None,
        }
    }

    /// Sets the default option.
    #[must_use]
    pub fn with_default(mut self, default: char) -> Self {
        self.default = Some(default);
        self
    }

    /// Displays the prompt and waits for user input.
    ///
    /// Returns the selected option, or an error if the prompt was cancelled or
    /// another error occurred.
    pub fn prompt(&self, writer: &mut dyn io::Write) -> Result<char, InquireError> {
        let mut option_keys: Vec<char> = self.options.iter().map(|o| o.key).collect();
        option_keys.push('?');

        let help_text = self
            .build_help_text()
            .map_err(|e| InquireError::Custom(Box::new(e)))?;

        let message = format!(
            "{} [{}]",
            self.message,
            option_keys
                .iter()
                .map(char::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );

        loop {
            let handler = CustomType::<char> {
                message: &message,
                starting_input: None,
                default: self.default,
                placeholder: None,
                help_message: None,
                formatter: &|c| c.to_string(),
                default_value_formatter: &|c| c.to_string(),
                parser: &|input: &str| {
                    let first_char = input.trim().chars().next().ok_or(())?;
                    if option_keys.contains(&first_char) {
                        return Ok(first_char);
                    }

                    Err(())
                },
                validators: vec![],
                error_message: format!("Invalid option:\n{help_text}"),
                render_config: RenderConfig::default(),
                submit_on_valid_parse: true,
            };

            let _unused = match handler.prompt_with_writer(writer)? {
                '?' => writeln!(writer, "{help_text}"),
                c => return Ok(c),
            };
        }
    }

    /// Builds the help text from the options list.
    fn build_help_text(&self) -> Result<String, fmt::Error> {
        let mut buf = String::new();
        for opt in &self.options {
            writeln!(buf, "{} - {}", opt.key, opt.description)?;
        }

        write!(buf, "? - print help")?;
        Ok(buf)
    }
}

#[cfg(test)]
#[path = "inline_select_tests.rs"]
mod tests;
