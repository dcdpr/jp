//! Interactive prompts for tool execution.
//!
//! The `ToolPrompter` handles all interactive prompts during tool execution:
//!
//! - Permission prompts (run/skip/edit)
//! - Question prompts (tool-specific input)
//! - Result edit prompts
//!
//! This keeps all I/O in the CLI layer, following the "functional core,
//! imperative shell" principle.
//! The `jp_llm` crate remains pure.

use std::{io::Write as _, sync::Arc};

use crossterm::style::Stylize as _;
use jp_config::conversation::tool::{RunMode, ToolSource};
use jp_conversation::event::SelectOption;
use jp_editor::{EditOutcome, EditorBackend};
use jp_inquire::{InlineOption, ReplyEditMode, ReplyOutcome, prompt::PromptBackend};
use jp_llm::tool::executor::PermissionInfo;
use jp_printer::Printer;
use jp_tool::AnswerType;
use serde_json::Value;

use crate::{Error, editor::report_editor_failure};

/// Result of a permission prompt.
#[derive(Debug)]
pub enum PermissionResult {
    /// User approved running the tool.
    Run {
        /// The arguments to use (possibly modified if user edited them).
        arguments: Value,
        /// Whether to remember this decision for the rest of the turn.
        persist: bool,
    },

    /// User skipped the tool.
    Skip {
        /// Optional reason for skipping.
        reason: Option<String>,
        /// Whether to remember this decision for the rest of the turn.
        persist: bool,
    },
}

/// Result of an argument edit operation.
#[derive(Debug)]
enum EditResult {
    /// User edited and saved valid JSON.
    Edited(Value),

    /// User emptied the content (signals fallback to Ask).
    Emptied,

    /// User cancelled the edit (`Ctrl+C`).
    Cancelled,
}

/// Outcome of an inline edit (the reply widget plus the `Ctrl+X` editor
/// escape).
enum InlineEditResult {
    /// User pressed Enter; carries the buffer (possibly empty).
    Submitted(String),

    /// User cancelled with `Ctrl+C`.
    Cancelled,
}

/// Handles interactive prompts for tool execution.
///
/// This struct centralizes all interactive I/O for tools, allowing the
/// coordinator to track when prompts are active.
///
/// Uses type-erased backends (`Arc<dyn ...>`) to allow runtime injection of
/// mock backends for testing.
pub struct ToolPrompter {
    /// Editor backend for the `Ctrl+X` escape from the inline editor.
    /// `None` when no editor is configured; the inline widget still works.
    editor: Option<Arc<dyn EditorBackend>>,

    /// Prompt backend for interactive prompts.
    prompt_backend: Arc<dyn PromptBackend>,

    /// Editing style for the inline reply widget used by the edit prompts.
    edit_mode: ReplyEditMode,

    printer: Arc<Printer>,
}

impl ToolPrompter {
    /// Creates a new tool prompter with a custom prompt backend.
    ///
    /// This allows injecting a mock prompt backend for testing while still
    /// using the real editor backend.
    pub fn with_prompt_backend(
        printer: Arc<Printer>,
        editor: Option<Arc<dyn EditorBackend>>,
        prompt_backend: Arc<dyn PromptBackend>,
        edit_mode: ReplyEditMode,
    ) -> Self {
        Self {
            editor,
            prompt_backend,
            edit_mode,
            printer,
        }
    }

    /// Creates a prompter with custom backends.
    #[cfg(test)]
    pub fn with_backends(
        printer: Arc<Printer>,
        editor: Option<Arc<dyn EditorBackend>>,
        prompt_backend: Arc<dyn PromptBackend>,
    ) -> Self {
        Self {
            editor,
            prompt_backend,
            edit_mode: ReplyEditMode::Emacs,
            printer,
        }
    }

    /// Prompts the user for permission to run a tool.
    ///
    /// Based on the `run_mode`, this may:
    ///
    /// - Show an interactive prompt (Ask)
    /// - Edit the arguments inline (Edit)
    /// - Return immediately (Unattended)
    ///
    /// # Returns
    ///
    /// - `PermissionResult::Run` if the user approved (with possibly modified
    ///   args)
    /// - `PermissionResult::Skip` if the user declined
    pub fn prompt_permission(&self, info: &PermissionInfo) -> Result<PermissionResult, Error> {
        match info.run_mode {
            RunMode::Unattended => Ok(PermissionResult::Run {
                arguments: info.arguments.clone(),
                persist: false,
            }),

            RunMode::Ask => {
                self.prompt_ask(&info.tool_name, &info.tool_source, info.arguments.clone())
            }

            RunMode::Edit => {
                self.prompt_edit(&info.tool_name, &info.tool_source, info.arguments.clone())
            }

            RunMode::Skip => Ok(PermissionResult::Skip {
                reason: None,
                persist: false,
            }),
        }
    }

    /// Builds the select options for the permission prompt.
    ///
    /// Returns `SelectOption`s that can be rendered as an inline select.
    fn permission_options() -> Vec<SelectOption> {
        // `r` (skip & reply) and `e` (edit arguments) drive the inline reply
        // widget, which needs only a tty — they no longer require a configured
        // editor (the `Ctrl+X` escape does, but it is a no-op without one).
        vec![
            SelectOption::new("y", "Run tool"),
            SelectOption::new("Y", "Run tool, remember for this turn"),
            SelectOption::new("n", "Skip running tool"),
            SelectOption::new("N", "Skip running tool, remember for this turn"),
            SelectOption::new("p", "Print arguments as JSON"),
            SelectOption::new("r", "Skip and reply"),
            SelectOption::new("e", "Edit arguments"),
        ]
    }

    /// Shows the interactive permission prompt.
    fn prompt_ask(
        &self,
        tool_name: &str,
        tool_source: &ToolSource,
        arguments: Value,
    ) -> Result<PermissionResult, Error> {
        let current_args = arguments;

        loop {
            let question = build_permission_question(tool_name, tool_source);

            let inline_options = select_options_to_inline(&Self::permission_options());

            let mut writer = self.printer.prompt_writer();

            match self
                .prompt_backend
                .inline_select(&question, inline_options, None, &mut writer)
            {
                Ok('y') => {
                    return Ok(PermissionResult::Run {
                        arguments: current_args,
                        persist: false,
                    });
                }
                Ok('Y') => {
                    return Ok(PermissionResult::Run {
                        arguments: current_args,
                        persist: true,
                    });
                }
                Ok('n') => {
                    return Ok(PermissionResult::Skip {
                        reason: None,
                        persist: false,
                    });
                }
                Ok('N') => {
                    return Ok(PermissionResult::Skip {
                        reason: None,
                        persist: true,
                    });
                }
                Ok('r') => {
                    let reason =
                        self.edit_text("_Provide reasoning for skipping tool execution_")?;
                    return Ok(PermissionResult::Skip {
                        reason,
                        persist: false,
                    });
                }
                Ok('p') => {
                    // Print raw JSON arguments
                    let json = serde_json::to_string_pretty(&current_args)
                        .unwrap_or_else(|_| format!("{current_args}"));
                    drop(writeln!(writer, "\n```json\n{json}\n```"));
                    // Loop back to prompt
                }
                Ok('e') => {
                    match self.try_edit_arguments(&current_args)? {
                        EditResult::Edited(edited) => {
                            return Ok(PermissionResult::Run {
                                arguments: edited,
                                persist: false,
                            });
                        }
                        EditResult::Emptied => {
                            // Loop back to ask
                        }
                        EditResult::Cancelled => {
                            return Ok(PermissionResult::Skip {
                                reason: Some("Edit cancelled".to_string()),
                                persist: false,
                            });
                        }
                    }
                }
                Ok(_) | Err(_) => {
                    // inquire doesn't add a newline on cancellation, leaving
                    // the cursor on the prompt line. Clear it so subsequent
                    // output (waiting indicator, interrupt menu) doesn't
                    // collide with residual text.
                    drop(write!(writer, "\r\x1b[K"));
                    return Ok(PermissionResult::Skip {
                        reason: None,
                        persist: false,
                    });
                }
            }
        }
    }

    /// Edits the tool arguments inline before running.
    ///
    /// If the user empties the content, falls back to Ask mode.
    /// Invalid JSON re-prompts the inline editor with the error in the prompt
    /// line.
    fn prompt_edit(
        &self,
        tool_name: &str,
        tool_source: &ToolSource,
        arguments: Value,
    ) -> Result<PermissionResult, Error> {
        match self.try_edit_arguments(&arguments)? {
            EditResult::Edited(edited) => Ok(PermissionResult::Run {
                arguments: edited,
                persist: false,
            }),
            EditResult::Emptied => self.prompt_ask(tool_name, tool_source, arguments),
            EditResult::Cancelled => Ok(PermissionResult::Skip {
                reason: Some("Edit cancelled".to_string()),
                persist: false,
            }),
        }
    }

    /// Edit JSON arguments inline, re-prompting on parse errors.
    ///
    /// The buffer is seeded with the pretty-printed arguments.
    /// `Ctrl+X` escapes to the configured editor; an emptied buffer abandons
    /// the edit (fall back to Ask), and a cancel abandons it as cancelled.
    fn try_edit_arguments(&self, arguments: &Value) -> Result<EditResult, Error> {
        let mut text = serde_json::to_string_pretty(arguments)
            .map_err(|e| Error::Editor(format!("Failed to serialize arguments: {e}")))?;
        let mut message = "Edit arguments".to_owned();

        loop {
            match self.inline_edit(&message, &text)? {
                InlineEditResult::Cancelled => return Ok(EditResult::Cancelled),
                InlineEditResult::Submitted(edited) => {
                    if edited.trim().is_empty() {
                        return Ok(EditResult::Emptied);
                    }
                    match serde_json::from_str::<Value>(&edited) {
                        Ok(value) => return Ok(EditResult::Edited(value)),
                        Err(e) => {
                            // Re-seed with the user's text and surface the error
                            // in the prompt line — no process re-spawn.
                            text = edited;
                            message = format!("Invalid JSON: {e}");
                        }
                    }
                }
            }
        }
    }

    /// Collect a free-text reason for skipping a tool.
    ///
    /// The buffer is seeded with `placeholder`.
    /// Submitting it unchanged, an empty buffer, or a cancel all yield `None`
    /// (skip with no reason).
    fn edit_text(&self, placeholder: &str) -> Result<Option<String>, Error> {
        match self.inline_edit("Reason for skipping (optional)", placeholder)? {
            InlineEditResult::Cancelled => Ok(None),
            InlineEditResult::Submitted(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() || trimmed == placeholder {
                    Ok(None)
                } else {
                    Ok(Some(content))
                }
            }
        }
    }

    /// Edit a tool result inline before delivery to the LLM.
    ///
    /// # Returns
    ///
    /// - `Some(edited_result)` if the user submitted non-empty text.
    /// - `None` if the user emptied the content or cancelled (caller falls back
    ///   to Ask).
    pub fn edit_result(&self, result: &str) -> Result<Option<String>, Error> {
        match self.inline_edit("Edit result before delivery", result)? {
            InlineEditResult::Cancelled => Ok(None),
            InlineEditResult::Submitted(content) => {
                if content.trim().is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(content))
                }
            }
        }
    }

    /// Run the inline reply widget seeded with `seed`, handling the `Ctrl+X`
    /// editor escape internally (re-seeding the widget with the editor's
    /// output).
    ///
    /// Returns when the user submits or cancels.
    /// With no editor configured the escape is a no-op and the widget stays
    /// open.
    fn inline_edit(&self, message: &str, seed: &str) -> Result<InlineEditResult, Error> {
        let mut text = seed.to_owned();
        loop {
            let output = self.printer.owned_prompt_writer();
            match self
                .prompt_backend
                .inline_reply(message, &text, self.edit_mode, true, output)
                .map_err(|error| Error::Editor(error.to_string()))?
            {
                ReplyOutcome::Submit(content) => return Ok(InlineEditResult::Submitted(content)),
                ReplyOutcome::Cancelled => return Ok(InlineEditResult::Cancelled),
                ReplyOutcome::OpenEditor { current_text } => {
                    text = current_text;
                    if let Some(editor) = &self.editor {
                        match editor.edit_text(&text) {
                            Ok((EditOutcome::Saved, edited)) => text = edited,
                            // Editor cancelled: keep the buffer and re-prompt.
                            Ok((EditOutcome::Cancelled, _)) => {}
                            // A spawn / I/O failure must not abort the whole
                            // prompt: surface it (chrome + diagnostics) and keep
                            // the buffer, re-prompting the inline widget.
                            Err(error) => {
                                report_editor_failure(&self.printer, &error, "Keeping your text.");
                            }
                        }
                    }
                }
            }
        }
    }

    /// Prompts the user for confirmation before delivering tool result to LLM.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if user confirms delivery
    /// - `Ok(false)` if user skips delivery
    pub fn prompt_result_confirmation(&self, tool_name: &str) -> Result<bool, Error> {
        let mut writer = self.printer.prompt_writer();

        let question = format!("Deliver {} result to assistant?", tool_name.yellow().bold());

        // "Edit result first" uses the inline widget (any tty); it no longer
        // requires a configured editor.
        let options = vec![
            InlineOption::new('y', "Deliver result"),
            InlineOption::new('n', "Skip delivery"),
            InlineOption::new('e', "Edit result first"),
        ];

        match self
            .prompt_backend
            .inline_select(&question, options, Some('y'), &mut writer)
        {
            Ok('y') => Ok(true),
            Ok('e') => {
                // Signal that user wants to edit - caller should handle
                Err(Error::Editor("edit_requested".to_string()))
            }
            Ok(_) | Err(_) => {
                drop(write!(writer, "\r\x1b[K"));
                Ok(false)
            }
        }
    }

    /// Prompts the user for a tool-specific question.
    ///
    /// This handles different question types:
    ///
    /// - Boolean (yes/no with optional persistence)
    /// - Select (choose from options)
    /// - Text (free text input)
    ///
    /// # Returns
    ///
    /// A `QuestionResult` containing the answer and `persist_level` which
    /// indicates whether the answer should be remembered for this turn.
    pub fn prompt_question(&self, question: &jp_tool::Question) -> Result<QuestionResult, Error> {
        let mut writer = self.printer.prompt_writer();

        if let Some(pre_amble) = &question.pre_amble {
            writeln!(writer, "{pre_amble}")?;
        }

        match &question.answer_type {
            AnswerType::Boolean => self.prompt_boolean_git_style(question, &mut writer),
            AnswerType::Select { options } => {
                let default_idx = question
                    .default
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .and_then(|def| options.iter().position(|opt| opt == def));

                let answer = self.prompt_backend.select(
                    &question.text,
                    options.clone(),
                    default_idx,
                    &mut writer,
                )?;

                Ok(QuestionResult {
                    answer: Value::String(answer),
                    persist_level: jp_tool::PersistLevel::None,
                })
            }
            AnswerType::Text => {
                let default_str = question.default.as_ref().and_then(|v| v.as_str());

                let answer = self
                    .prompt_backend
                    .text(&question.text, default_str, &mut writer)?;

                Ok(QuestionResult {
                    answer: Value::String(answer),
                    persist_level: jp_tool::PersistLevel::None,
                })
            }
            AnswerType::Secret => {
                let answer = self.prompt_backend.password(&question.text, &mut writer)?;

                Ok(QuestionResult {
                    answer: Value::String(answer),
                    persist_level: jp_tool::PersistLevel::None,
                })
            }
        }
    }

    /// Prompts for a boolean answer with git-style options.
    ///
    /// Options:
    ///
    /// - `y` = yes, just this once
    /// - `Y` = yes, and remember for this turn
    /// - `n` = no, just this once
    /// - `N` = no, and remember for this turn
    fn prompt_boolean_git_style(
        &self,
        question: &jp_tool::Question,
        writer: &mut jp_printer::PrinterWriter<'_>,
    ) -> Result<QuestionResult, Error> {
        let options = vec![
            InlineOption::new('y', "yes, just this once"),
            InlineOption::new('Y', "yes, and remember for this turn"),
            InlineOption::new('n', "no, just this once"),
            InlineOption::new('N', "no, and remember for this turn"),
        ];

        let default_char = question
            .default
            .as_ref()
            .and_then(serde_json::Value::as_bool)
            .map(|b| if b { 'y' } else { 'n' });

        let answer =
            self.prompt_backend
                .inline_select(&question.text, options, default_char, writer)?;

        match answer {
            'y' => Ok(QuestionResult {
                answer: Value::Bool(true),
                persist_level: jp_tool::PersistLevel::None,
            }),
            'Y' => Ok(QuestionResult {
                answer: Value::Bool(true),
                persist_level: jp_tool::PersistLevel::Turn,
            }),
            'n' => Ok(QuestionResult {
                answer: Value::Bool(false),
                persist_level: jp_tool::PersistLevel::None,
            }),
            'N' => Ok(QuestionResult {
                answer: Value::Bool(false),
                persist_level: jp_tool::PersistLevel::Turn,
            }),
            _ => unreachable!(),
        }
    }
}

/// Builds the permission question string.
fn build_permission_question(tool_name: &str, tool_source: &ToolSource) -> String {
    let source_type = match tool_source {
        ToolSource::Builtin { .. } => "built-in",
        ToolSource::Local { .. } => "local",
        ToolSource::Mcp { .. } => "mcp",
    };

    let mut question = format!("Run {} {} tool", source_type, tool_name.yellow().bold());

    if let ToolSource::Mcp { server, .. } = tool_source {
        question = format!(
            "{} from {} server?",
            question,
            server.as_str().blue().bold()
        );
    } else {
        question.push('?');
    }

    question
}

/// Converts `SelectOption`s to `InlineOption`s for the prompt backend.
///
/// Each option's value must be a single-character string (used as the key).
/// The description becomes the help text.
/// Options with non-single-char values are skipped.
fn select_options_to_inline(options: &[SelectOption]) -> Vec<InlineOption> {
    options
        .iter()
        .filter_map(|opt| {
            let key = opt.value.as_str()?;
            let ch = single_char(key)?;
            let desc = opt.description.as_deref().unwrap_or(key);
            Some(InlineOption::new(ch, desc))
        })
        .collect()
}

/// Returns the char if the string is exactly one character.
fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(ch)
}

/// Result of a question prompt.
#[derive(Debug)]
pub struct QuestionResult {
    /// The answer value.
    pub answer: Value,
    /// Whether to persist the answer for this turn.
    pub persist_level: jp_tool::PersistLevel,
}

#[cfg(test)]
#[path = "prompter_tests.rs"]
mod tests;
