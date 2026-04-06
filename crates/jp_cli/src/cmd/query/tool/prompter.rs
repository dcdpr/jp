//! Interactive prompts for tool execution.
//!
//! The `ToolPrompter` handles all interactive prompts during tool execution:
//! - Permission prompts (run/skip/edit)
//! - Question prompts (tool-specific input)
//! - Result edit prompts
//!
//! This keeps all I/O in the CLI layer, following the "functional core,
//! imperative shell" principle. The `jp_llm` crate remains pure.

use std::{io::Write as _, sync::Arc};

use camino::Utf8PathBuf;
use crossterm::style::Stylize as _;
use jp_config::conversation::tool::{RunMode, ToolSource};
use jp_conversation::event::{InquiryId, SelectOption};
use jp_editor::{EditorBackend, TerminalEditorBackend};
use jp_inquire::{InlineOption, prompt::PromptBackend};
use jp_llm::tool::executor::PermissionInfo;
use jp_mcp::{
    Client,
    id::{McpServerId, McpToolId},
};
use jp_printer::Printer;
use jp_tool::AnswerType;
use serde_json::Value;

use crate::Error;

/// Well-known question ID used for tool permission inquiries.
pub const PERMISSION_QUESTION_ID: &str = "__permission__";

/// Constructs a stable `InquiryId` for a tool's permission prompt.
///
/// Format: `"<tool_name>.__permission__"`. This is stable across
/// different invocations of the same tool within a turn.
#[must_use]
pub fn permission_inquiry_id(tool_name: &str) -> InquiryId {
    InquiryId::new(format!("{tool_name}.{PERMISSION_QUESTION_ID}"))
}

/// Constructs a stable `InquiryId` for a tool question.
///
/// Format: `"<tool_name>.<question_id>"`. This is stable across
/// different invocations of the same tool within a turn.
#[must_use]
pub fn tool_question_inquiry_id(tool_name: &str, question_id: &str) -> InquiryId {
    InquiryId::new(format!("{tool_name}.{question_id}"))
}

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

    /// User cancelled the edit (e.g., JSON error + declined retry).
    Cancelled,
}

/// Handles interactive prompts for tool execution.
///
/// This struct centralizes all interactive I/O for tools, allowing the
/// coordinator to track when prompts are active.
///
/// Uses type-erased backends (`Arc<dyn ...>`) to allow runtime injection
/// of mock backends for testing.
pub struct ToolPrompter {
    /// Editor backend for edit modes. If `None`, edit mode falls back to Ask.
    editor: Option<Arc<dyn EditorBackend>>,

    /// Prompt backend for interactive prompts.
    prompt_backend: Arc<dyn PromptBackend>,

    printer: Arc<Printer>,
}

impl ToolPrompter {
    /// Creates a new tool prompter with a custom prompt backend.
    ///
    /// This allows injecting a mock prompt backend for testing while still
    /// using the real editor backend.
    pub fn with_prompt_backend(
        printer: Arc<Printer>,
        editor_path: Option<Utf8PathBuf>,
        prompt_backend: Arc<dyn PromptBackend>,
    ) -> Self {
        let editor = editor_path
            .map(|path| Arc::new(TerminalEditorBackend { path }) as Arc<dyn EditorBackend>);

        Self {
            editor,
            prompt_backend,
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
            printer,
        }
    }

    /// Creates a prompter with a custom editor backend.
    #[cfg(test)]
    pub fn with_editor_backend(
        printer: Arc<Printer>,
        backend: impl EditorBackend + 'static,
    ) -> Self {
        use jp_inquire::prompt::TerminalPromptBackend;

        Self {
            editor: Some(Arc::new(backend)),
            prompt_backend: Arc::new(TerminalPromptBackend),
            printer,
        }
    }

    /// Prompts the user for permission to run a tool.
    ///
    /// Based on the `run_mode`, this may:
    /// - Show an interactive prompt (Ask)
    /// - Open an editor for arguments (Edit)
    /// - Return immediately (Unattended)
    ///
    /// # Returns
    ///
    /// - `PermissionResult::Run` if the user approved (with possibly modified args)
    /// - `PermissionResult::Skip` if the user declined
    pub async fn prompt_permission(
        &self,
        info: &PermissionInfo,
        mcp_client: &Client,
    ) -> Result<PermissionResult, Error> {
        match info.run_mode {
            RunMode::Unattended => Ok(PermissionResult::Run {
                arguments: info.arguments.clone(),
                persist: false,
            }),

            RunMode::Ask => {
                self.prompt_ask(
                    &info.tool_name,
                    &info.tool_source,
                    info.arguments.clone(),
                    mcp_client,
                )
                .await
            }

            RunMode::Edit => {
                self.prompt_edit(
                    &info.tool_name,
                    &info.tool_source,
                    info.arguments.clone(),
                    mcp_client,
                )
                .await
            }

            RunMode::Skip => Ok(PermissionResult::Skip {
                reason: None,
                persist: false,
            }),
        }
    }

    /// Builds the select options for the permission prompt.
    ///
    /// The available options depend on whether an editor is configured.
    /// Returns `SelectOption`s that can be rendered as an inline select.
    fn permission_options(&self) -> Vec<SelectOption> {
        let mut opts = vec![
            SelectOption::new("y", "Run tool"),
            SelectOption::new("Y", "Run tool, remember for this turn"),
            SelectOption::new("n", "Skip running tool"),
            SelectOption::new("N", "Skip running tool, remember for this turn"),
            SelectOption::new("p", "Print arguments as JSON"),
        ];

        if self.editor.is_some() {
            opts.push(SelectOption::new("r", "Skip and reply"));
            opts.push(SelectOption::new("e", "Edit arguments"));
        }

        opts
    }

    /// Shows the interactive permission prompt.
    async fn prompt_ask(
        &self,
        tool_name: &str,
        tool_source: &ToolSource,
        arguments: Value,
        mcp_client: &Client,
    ) -> Result<PermissionResult, Error> {
        let current_args = arguments;

        loop {
            let question = self
                .build_permission_question(tool_name, tool_source, mcp_client)
                .await?;

            let inline_options = select_options_to_inline(&self.permission_options());

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

    /// Opens an editor for the user to modify tool arguments.
    ///
    /// If no editor is configured, falls back to Ask mode.
    /// If the user empties the content, falls back to Ask mode.
    /// If JSON parsing fails, prompts to retry or fail.
    async fn prompt_edit(
        &self,
        tool_name: &str,
        tool_source: &ToolSource,
        arguments: Value,
        mcp_client: &Client,
    ) -> Result<PermissionResult, Error> {
        let Some(_) = &self.editor else {
            return self
                .prompt_ask(tool_name, tool_source, arguments, mcp_client)
                .await;
        };

        match self.try_edit_arguments(&arguments)? {
            EditResult::Edited(edited) => Ok(PermissionResult::Run {
                arguments: edited,
                persist: false,
            }),
            EditResult::Emptied => {
                self.prompt_ask(tool_name, tool_source, arguments, mcp_client)
                    .await
            }
            EditResult::Cancelled => Ok(PermissionResult::Skip {
                reason: Some("Edit cancelled".to_string()),
                persist: false,
            }),
        }
    }

    /// Attempts to edit arguments in an editor.
    ///
    /// Returns the edit result without any async recursion.
    fn try_edit_arguments(&self, arguments: &Value) -> Result<EditResult, Error> {
        let Some(editor) = &self.editor else {
            return Err(Error::Editor("No editor configured".to_string()));
        };

        let mut json = serde_json::to_string_pretty(arguments)
            .map_err(|e| Error::Editor(format!("Failed to serialize arguments: {e}")))?;

        loop {
            json = editor
                .edit(&json)
                .map_err(|error| Error::Editor(error.to_string()))?;

            if json.trim().is_empty() {
                return Ok(EditResult::Emptied);
            }

            match serde_json::from_str::<Value>(&json) {
                Ok(edited) => return Ok(EditResult::Edited(edited)),
                Err(e) => {
                    let mut writer = self.printer.prompt_writer();
                    drop(writeln!(writer, "JSON parsing error: {e}"));

                    let options = vec![
                        InlineOption::new('y', "Open editor to fix arguments"),
                        InlineOption::new('n', "Cancel edit"),
                    ];

                    let retry = self
                        .prompt_backend
                        .inline_select("Re-open editor?", options, Some('n'), &mut writer)
                        .unwrap_or('n');

                    if retry == 'n' {
                        return Ok(EditResult::Cancelled);
                    }
                }
            }
        }
    }

    /// Opens an editor for the user to provide reasoning for skipping tool execution.
    ///
    /// The editor is pre-populated with a placeholder prompt. If the user empties
    /// the content or leaves only the placeholder, a default reason is returned.
    fn edit_text(&self, placeholder: &str) -> Result<Option<String>, Error> {
        let Some(editor) = &self.editor else {
            return Err(Error::Editor("No editor configured".to_string()));
        };

        let content = editor
            .edit(placeholder)
            .map_err(|error| Error::Editor(error.to_string()))?;

        let trimmed = content.trim();
        if trimmed.is_empty() || trimmed == placeholder {
            Ok(None)
        } else {
            Ok(Some(content))
        }
    }

    /// Opens an editor for the user to modify tool result before delivery to LLM.
    ///
    /// # Returns
    ///
    /// - `Some(edited_result)` if user edited and saved
    /// - `None` if user emptied content (caller should fall back to Ask)
    ///
    /// # Errors
    ///
    /// Returns an error if no editor is configured or the editor fails.
    pub fn edit_result(&self, result: &str) -> Result<Option<String>, Error> {
        let Some(editor) = &self.editor else {
            return Err(Error::Editor("No editor configured".to_string()));
        };

        let content = editor
            .edit(result)
            .map_err(|error| Error::Editor(error.to_string()))?;

        if content.trim().is_empty() {
            return Ok(None);
        }

        Ok(Some(content))
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

        let options = if self.editor.is_some() {
            vec![
                InlineOption::new('y', "Deliver result"),
                InlineOption::new('n', "Skip delivery"),
                InlineOption::new('e', "Edit result first"),
            ]
        } else {
            vec![
                InlineOption::new('y', "Deliver result"),
                InlineOption::new('n', "Skip delivery"),
            ]
        };

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

    /// Returns whether an editor is configured.
    pub fn has_editor(&self) -> bool {
        self.editor.is_some()
    }

    /// Builds the permission question string.
    async fn build_permission_question(
        &self,
        tool_name: &str,
        tool_source: &ToolSource,
        mcp_client: &Client,
    ) -> Result<String, Error> {
        let source_type = match tool_source {
            ToolSource::Builtin { .. } => "built-in",
            ToolSource::Local { .. } => "local",
            ToolSource::Mcp { .. } => "mcp",
        };

        let mut question = format!("Run {} {} tool", source_type, tool_name.yellow().bold());

        if let ToolSource::Mcp { server, tool } = tool_source {
            let tool_id = McpToolId::new(tool.as_ref().unwrap_or(&tool_name.to_string()));
            let server_id = server.as_ref().map(|s| McpServerId::new(s.clone()));

            if let Ok(resolved_server_id) = mcp_client
                .get_tool_server_id(&tool_id, server_id.as_ref())
                .await
            {
                question = format!(
                    "{} from {} server?",
                    question,
                    resolved_server_id.as_str().blue().bold()
                );
            }
        } else {
            question.push('?');
        }

        Ok(question)
    }

    /// Prompts the user for a tool-specific question.
    ///
    /// This handles different question types:
    /// - Boolean (yes/no with optional persistence)
    /// - Select (choose from options)
    /// - Text (free text input)
    ///
    /// # Returns
    ///
    /// A `QuestionResult` containing the answer and `persist_level` which indicates
    /// whether the answer should be remembered for this turn.
    pub fn prompt_question(&self, question: &jp_tool::Question) -> Result<QuestionResult, Error> {
        let mut writer = self.printer.prompt_writer();

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
        }
    }

    /// Prompts for a boolean answer with git-style options.
    ///
    /// Options:
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

/// Converts `SelectOption`s to `InlineOption`s for the prompt backend.
///
/// Each option's value must be a single-character string (used as the key).
/// The description becomes the help text. Options with non-single-char values
/// are skipped.
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
