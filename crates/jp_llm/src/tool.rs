//! Tool call utilities.

pub mod builtin;
pub mod executor;

use std::{ffi::OsStr, process::Stdio, sync::Arc};

pub use builtin::BuiltinTool;
use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::conversation::tool::{
    OneOrManyTypes, ToolCommandConfig, ToolConfigWithDefaults, ToolParameterConfig, ToolSource,
};
use jp_conversation::event::ToolCallResponse;
use jp_mcp::{
    RawContent, ResourceContents,
    id::{McpServerId, McpToolId},
};
use jp_tool::{Action, Outcome, Question};
use minijinja::Environment;
use serde_json::{Map, Value, json};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use tracing::{info, trace};

use crate::error::ToolError;

/// Documentation for a single tool parameter.
#[derive(Debug, Clone)]
pub struct ParameterDocs {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub examples: Option<String>,
}

impl ParameterDocs {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.description.is_none() && self.examples.is_none()
    }
}

/// Documentation for a single tool.
#[derive(Debug, Clone)]
pub struct ToolDocs {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub examples: Option<String>,
    pub parameters: IndexMap<String, ParameterDocs>,
}

impl ToolDocs {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.description.is_none()
            && self.examples.is_none()
            && self.parameters.values().all(ParameterDocs::is_empty)
    }
}

/// The outcome of a tool execution.
///
/// This type represents the possible results of executing a tool's underlying
/// command or MCP call, without any interactive prompts. The caller is
/// responsible for:
///
/// 1. Handling permission prompts **before** calling
///    [`ToolDefinition::execute()`].
/// 2. Handling [`ExecutionOutcome::NeedsInput`] by prompting the user or
///    assistant.
/// 3. Handling result editing **after** receiving the outcome.
///
/// # Example Flow
///
/// ```text
/// ToolExecutor (jp_cli)                    ToolDefinition (jp_llm)
/// ─────────────────────                    ──────────────────────
///        │
///        ├── [AwaitingPermission]
///        │   prompt_permission()
///        │
///        ├── [Running]
///        │   ────────────────────────────► execute()
///        │                                      │
///        │   ◄──────────────────────────── ExecutionOutcome
///        ├── [AwaitingInput] (if NeedsInput)
///        │   prompt_question()
///        │   ────────────────────────────► execute() (with answer)
///        │                                      │
///        │   ◄──────────────────────────── ExecutionOutcome
///        ├── [AwaitingResultEdit]
///        │   prompt_result_edit()
///        │
///        └── [Completed]
/// ```
#[derive(Debug)]
pub enum ExecutionOutcome {
    /// Tool executed and produced a result.
    Completed {
        /// The tool call ID (for correlation with the request).
        id: String,

        /// The execution result.
        ///
        /// If an error occurred, it means the tool ran, but reported an error.
        result: Result<String, String>,
    },

    /// Tool needs additional input before it can complete.
    ///
    /// The caller should:
    /// 1. Present the question to the user (or delegate to the assistant)
    /// 2. Collect the answer
    /// 3. Call [`ToolDefinition::execute()`] again with the answer in `answers`
    NeedsInput {
        /// The tool call ID.
        id: String,

        /// The question to ask.
        question: Question,
    },

    /// Tool execution was cancelled via the cancellation token.
    ///
    /// This occurs when the user interrupts tool execution (e.g., Ctrl+C during
    /// a long-running command).
    Cancelled {
        /// The tool call ID.
        id: String,
    },
}

impl ExecutionOutcome {
    /// Convert the outcome to a [`ToolCallResponse`].
    ///
    /// This is useful for building the final response to send to the LLM after
    /// any post-processing (e.g., result editing) is complete.
    ///
    /// # Note
    ///
    /// For [`ExecutionOutcome::NeedsInput`], this returns a placeholder
    /// response. The caller should typically handle `NeedsInput` specially
    /// rather than converting it directly to a response.
    #[must_use]
    pub fn into_response(self) -> ToolCallResponse {
        match self {
            Self::Completed { id, result } => ToolCallResponse { id, result },
            Self::NeedsInput { id, question } => ToolCallResponse {
                id,
                result: Ok(format!("Tool requires additional input: {}", question.text)),
            },
            Self::Cancelled { id } => ToolCallResponse {
                id,
                result: Ok("Tool execution cancelled by user.".to_string()),
            },
        }
    }

    /// Returns the tool call ID.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Completed { id, .. } | Self::NeedsInput { id, .. } | Self::Cancelled { id } => id,
        }
    }

    /// Returns `true` if this is a `NeedsInput` outcome.
    #[must_use]
    pub fn needs_input(&self) -> bool {
        matches!(self, Self::NeedsInput { .. })
    }

    /// Returns `true` if this is a `Cancelled` outcome.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled { .. })
    }

    /// Returns `true` if this is a `Completed` outcome with a successful result.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Completed { result: Ok(_), .. })
    }
}

/// Result of running a tool command.
///
/// This is the single parsing point for all tool command output. Both tool
/// execution and argument formatting go through this type, ensuring consistent
/// handling of `Outcome` variants (including error traces).
#[derive(Debug)]
pub enum CommandResult {
    /// Tool produced content.
    Success(String),

    /// Tool reported a transient error (can be retried).
    TransientError {
        /// The error message.
        message: String,

        /// The error trace (source chain from the tool process).
        trace: Vec<String>,
    },

    /// Tool reported a fatal error.
    FatalError(String),

    /// Tool needs additional input before it can continue.
    NeedsInput(Question),

    /// Tool was cancelled via the cancellation token.
    Cancelled,

    /// stdout wasn't valid `Outcome` JSON.
    ///
    /// Falls back to treating stdout as plain text. The `success` flag
    /// indicates the process exit status.
    RawOutput {
        /// Raw stdout content.
        stdout: String,

        /// Raw stderr content.
        stderr: String,

        /// Whether the process exited successfully.
        success: bool,
    },
}

impl CommandResult {
    /// Format a transient error message including trace details.
    ///
    /// If the trace is empty, returns just the message. Otherwise appends
    /// the trace entries so the LLM (or user) can see the root cause.
    #[must_use]
    pub fn format_error(message: &str, trace: &[String]) -> String {
        if trace.is_empty() {
            message.to_owned()
        } else {
            format!("{message}\n\nTrace:\n{}", trace.join("\n"))
        }
    }

    /// Convert to a `Result<String, String>` suitable for tool call responses.
    ///
    /// - `Success` → `Ok(content)`
    /// - `TransientError` → `Err(json with message + trace)`
    /// - `FatalError` → `Err(raw json)`
    /// - `NeedsInput` → handled separately by callers (this panics)
    /// - `Cancelled` → `Ok(cancellation message)`
    /// - `RawOutput` → `Ok(stdout)` if success, `Err(json)` if failure
    pub fn into_tool_result(self, name: &str) -> Result<String, String> {
        match self {
            Self::Success(content) => Ok(content),
            Self::TransientError { message, trace } => Err(json!({
                "message": message,
                "trace": trace,
            })
            .to_string()),
            Self::FatalError(raw) => Err(raw),
            Self::Cancelled => Ok("Tool execution cancelled by user.".to_string()),
            Self::RawOutput {
                stdout,
                stderr,
                success,
            } => {
                if success {
                    Ok(stdout)
                } else {
                    Err(json!({
                        "message": format!("Tool '{name}' execution failed."),
                        "stderr": stderr,
                        "stdout": stdout,
                    })
                    .to_string())
                }
            }
            Self::NeedsInput(_) => {
                unreachable!("NeedsInput should be handled by the caller")
            }
        }
    }
}

/// Run a tool command asynchronously with cancellation support.
///
/// This is the **single entry point** for running tool commands (both execution
/// and argument formatting). It handles:
///
/// 1. Template rendering via [`minijinja`]
/// 2. Process spawning via Tokio's [`Command`]
/// 3. Cancellation via [`CancellationToken`]
/// 4. Parsing stdout as [`jp_tool::Outcome`]
pub async fn run_tool_command(
    command: ToolCommandConfig,
    ctx: Value,
    root: &Utf8Path,
    cancellation_token: CancellationToken,
) -> Result<CommandResult, ToolError> {
    let ToolCommandConfig {
        program,
        args,
        shell,
    } = command;

    let tmpl = Arc::new(Environment::new());

    let program = tmpl
        .render_str(&program, &ctx)
        .map_err(|error| ToolError::TemplateError {
            data: program.clone(),
            error,
        })?;

    let args = args
        .iter()
        .map(|s| tmpl.render_str(s, &ctx))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ToolError::TemplateError {
            data: args.join(" "),
            error,
        })?;

    let mut cmd = if shell {
        let shell_cmd = std::iter::once(program.clone())
            .chain(args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&shell_cmd);
        cmd
    } else {
        let mut cmd = Command::new(&program);
        cmd.args(&args);
        cmd
    };

    let child = cmd
        .current_dir(root.as_std_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| ToolError::SpawnError {
            command: format!(
                "{} {}",
                cmd.as_std().get_program().to_string_lossy(),
                cmd.as_std()
                    .get_args()
                    .filter_map(OsStr::to_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            error,
        })?;

    let wait_handle = tokio::spawn(async move { child.wait_with_output().await });
    let abort_handle = wait_handle.abort_handle();

    tokio::select! {
        biased;
        () = cancellation_token.cancelled() => {
            abort_handle.abort();
            Ok(CommandResult::Cancelled)
        }
        result = wait_handle => {
            match result {
                Ok(Ok(output)) => Ok(parse_command_output(
                    &output.stdout,
                    &output.stderr,
                    output.status.success(),
                )),
                Ok(Err(error)) => Ok(CommandResult::RawOutput {
                    stdout: String::new(),
                    stderr: error.to_string(),
                    success: false,
                }),
                Err(join_error) => Ok(CommandResult::RawOutput {
                    stdout: String::new(),
                    stderr: format!("Task panicked: {join_error}"),
                    success: false,
                }),
            }
        }
    }
}

/// Parse raw command output into a [`CommandResult`].
///
/// Tries to deserialize stdout as [`jp_tool::Outcome`]. If that fails,
/// falls back to [`CommandResult::RawOutput`].
fn parse_command_output(stdout: &[u8], stderr: &[u8], success: bool) -> CommandResult {
    let stdout_str = String::from_utf8_lossy(stdout);

    match serde_json::from_str::<Outcome>(&stdout_str) {
        Ok(Outcome::Success { content }) => CommandResult::Success(content),
        Ok(Outcome::Error {
            transient,
            message,
            trace,
        }) => {
            if transient {
                CommandResult::TransientError { message, trace }
            } else {
                CommandResult::FatalError(stdout_str.into_owned())
            }
        }
        Ok(Outcome::NeedsInput { question }) => CommandResult::NeedsInput(question),
        Err(_) => CommandResult::RawOutput {
            stdout: stdout_str.into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
            success,
        },
    }
}

/// The definition of a tool.
///
/// The definition source is either a [`ToolConfig`] for `local` tools, or a
/// combination of `ToolConfig` and MCP server information for `mcp` tools, or
/// hard-coded for definitions `builtin` tools.
///
/// [`ToolConfig`]: jp_config::conversation::tool::ToolConfig
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: IndexMap<String, ToolParameterConfig>,
}

impl ToolDefinition {
    pub async fn new(
        name: &str,
        source: &ToolSource,
        description: Option<String>,
        parameters: IndexMap<String, ToolParameterConfig>,
        mcp_client: &jp_mcp::Client,
    ) -> Result<Self, ToolError> {
        match &source {
            ToolSource::Local { .. } | ToolSource::Builtin { .. } => Ok(local_tool_definition(
                name.to_owned(),
                description,
                parameters,
            )),
            ToolSource::Mcp { server, tool } => {
                mcp_tool_definition(
                    server.as_ref(),
                    name,
                    tool.as_deref(),
                    description,
                    parameters,
                    mcp_client,
                )
                .await
            }
        }
    }

    /// Execute the tool without any interactive prompts.
    ///
    /// This is a pure execution method that runs the tool's underlying command
    /// or MCP call and returns an [`ExecutionOutcome`]. All interactive
    /// decisions (permission prompts, result editing, question handling) are
    /// the caller's responsibility.
    ///
    /// # Arguments
    ///
    /// * `id` - The tool call ID for correlation with the request
    /// * `arguments` - The tool arguments (caller is responsible for any pre-processing)
    /// * `answers` - Pre-provided answers to tool questions (from previous `NeedsInput`)
    /// * `config` - Tool configuration
    /// * `mcp_client` - MCP client for MCP tool execution
    /// * `root` - Working directory for local tool execution
    /// * `cancellation_token` - Token to cancel long-running execution
    /// * `builtin_executors` - Registry of builtin tools
    ///
    /// # Returns
    ///
    /// - [`ExecutionOutcome::Completed`] - Tool finished (check inner `Result` for success/error)
    /// - [`ExecutionOutcome::NeedsInput`] - Tool needs user input to continue
    /// - [`ExecutionOutcome::Cancelled`] - Execution was cancelled via the token
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] for infrastructure errors (spawn failure, missing
    /// command, etc.). Tool-level errors (command returned non-zero) are
    /// returned as `Ok(ExecutionOutcome::Completed { result: Err(...) })`.
    ///
    /// # Example
    ///
    /// ```ignore
    /// loop {
    ///     match definition.execute(id, &args, &answers, ...).await? {
    ///         ExecutionOutcome::Completed { result, .. } => {
    ///             // Handle success or tool error
    ///             break result;
    ///         }
    ///         ExecutionOutcome::NeedsInput { question, .. } => {
    ///             // Prompt user for input
    ///             let answer = prompt_user(&question)?;
    ///             answers.insert(question.id, answer);
    ///             // Loop to retry with answer
    ///         }
    ///         ExecutionOutcome::Cancelled { .. } => {
    ///             break Ok("Cancelled".into());
    ///         }
    ///     }
    /// }
    /// ```
    pub async fn execute(
        &self,
        id: String,
        arguments: Value,
        answers: &IndexMap<String, Value>,
        config: &ToolConfigWithDefaults,
        mcp_client: &jp_mcp::Client,
        root: &Utf8Path,
        cancellation_token: CancellationToken,
        builtin_executors: &builtin::BuiltinExecutors,
    ) -> Result<ExecutionOutcome, ToolError> {
        info!(tool = %self.name, arguments = ?arguments, "Executing tool.");

        match config.source() {
            ToolSource::Local { tool } => {
                self.execute_local(
                    id,
                    arguments,
                    answers,
                    config,
                    tool.as_deref(),
                    root,
                    cancellation_token,
                )
                .await
            }
            ToolSource::Mcp { server, tool } => {
                self.execute_mcp(
                    id,
                    arguments,
                    mcp_client,
                    server.as_deref(),
                    tool.as_deref(),
                    cancellation_token,
                )
                .await
            }
            ToolSource::Builtin { .. } => {
                self.execute_builtin(id, &arguments, answers, builtin_executors)
                    .await
            }
        }
    }

    /// Execute a local tool and return the outcome.
    ///
    /// This is the pure execution path for local tools. It validates arguments,
    /// runs the command, and converts the result to an `ExecutionOutcome`.
    async fn execute_local(
        &self,
        id: String,
        mut arguments: Value,
        answers: &IndexMap<String, Value>,
        config: &ToolConfigWithDefaults,
        tool: Option<&str>,
        root: &Utf8Path,
        cancellation_token: CancellationToken,
    ) -> Result<ExecutionOutcome, ToolError> {
        let name = tool.unwrap_or(&self.name);

        // Apply configured defaults for missing parameters, then validate.
        if let Some(args) = arguments.as_object_mut() {
            apply_parameter_defaults(args, config.parameters());

            if let Err(error) = validate_tool_arguments(args, config.parameters()) {
                return Ok(ExecutionOutcome::Completed {
                    id,
                    result: Err(format!(
                        "Invalid arguments: {error}\n\nYou can call `describe_tools(tools: \
                         [\"{name}\"])` to learn more about how to use the tool correctly."
                    )),
                });
            }
        }

        let ctx = json!({
            "tool": {
                "name": name,
                "arguments": &arguments,
                "answers": answers,
            },
            "context": {
                "action": Action::Run,
                "root": root.as_str(),
            },
        });

        let Some(command) = config.command() else {
            return Err(ToolError::MissingCommand);
        };

        match run_tool_command(command, ctx, root, cancellation_token).await? {
            CommandResult::Success(content) => Ok(ExecutionOutcome::Completed {
                id,
                result: Ok(content),
            }),
            CommandResult::NeedsInput(question) => {
                Ok(ExecutionOutcome::NeedsInput { id, question })
            }
            CommandResult::Cancelled => Ok(ExecutionOutcome::Cancelled { id }),
            other => Ok(ExecutionOutcome::Completed {
                id,
                result: other.into_tool_result(name),
            }),
        }
    }

    /// Execute an MCP tool and return the outcome.
    ///
    /// This is the pure execution path for MCP tools. It calls the MCP server
    /// and converts the result to an `ExecutionOutcome`.
    async fn execute_mcp(
        &self,
        id: String,
        arguments: Value,
        mcp_client: &jp_mcp::Client,
        server: Option<&str>,
        tool: Option<&str>,
        cancellation_token: CancellationToken,
    ) -> Result<ExecutionOutcome, ToolError> {
        let name = tool.unwrap_or(&self.name);

        let call_future = mcp_client.call_tool(name, server, &arguments);

        tokio::select! {
            biased;
            () = cancellation_token.cancelled() => {
                info!(tool = %self.name, "MCP tool call cancelled");
                Ok(ExecutionOutcome::Cancelled { id })
            }
            result = call_future => {
                let result = result.map_err(ToolError::McpRunToolError)?;

                let content = result
                    .content
                    .into_iter()
                    .filter_map(|v| match v.raw {
                        RawContent::Text(v) => Some(v.text),
                        RawContent::Resource(v) => match v.resource {
                            ResourceContents::TextResourceContents { text, .. } => Some(text),
                            ResourceContents::BlobResourceContents { blob, .. } => Some(blob),
                        },
                        RawContent::Image(_) | RawContent::Audio(_) => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");

                let result = if result.is_error.unwrap_or_default() {
                    Err(content)
                } else {
                    Ok(content)
                };

                Ok(ExecutionOutcome::Completed { id, result })
            }
        }
    }

    /// Execute a builtin tool and return the outcome.
    async fn execute_builtin(
        &self,
        id: String,
        arguments: &Value,
        answers: &IndexMap<String, Value>,
        builtin_executors: &builtin::BuiltinExecutors,
    ) -> Result<ExecutionOutcome, ToolError> {
        let executor = builtin_executors
            .get(&self.name)
            .ok_or_else(|| ToolError::NotFound {
                name: self.name.clone(),
            })?;

        let outcome = executor.execute(arguments, answers).await;

        Ok(match outcome {
            jp_tool::Outcome::Success { content } => ExecutionOutcome::Completed {
                id,
                result: Ok(content),
            },
            jp_tool::Outcome::Error {
                message,
                trace,
                transient: _,
            } => {
                let error_msg = if trace.is_empty() {
                    message
                } else {
                    format!("{message}\n\nTrace:\n{}", trace.join("\n"))
                };
                ExecutionOutcome::Completed {
                    id,
                    result: Err(error_msg),
                }
            }
            jp_tool::Outcome::NeedsInput { question } => {
                ExecutionOutcome::NeedsInput { id, question }
            }
        })
    }

    /// Return a map of parameter names to JSON schemas.
    #[must_use]
    pub fn to_parameters_map(&self) -> Map<String, Value> {
        self.parameters
            .clone()
            .into_iter()
            .map(|(k, v)| (k, v.to_json_schema()))
            .collect()
    }

    /// Return a JSON schema for the parameters of the tool.
    #[must_use]
    pub fn to_parameters_schema(&self) -> Value {
        let required = self
            .parameters
            .iter()
            .filter(|(_, cfg)| cfg.required)
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();

        json!({
            "type": "object",
            "properties": self.to_parameters_map(),
            "additionalProperties": false,
            "required": required,
        })
    }
}

/// Split a description string into a short summary and remaining detail.
///
/// If the text is short (single line, ≤120 chars), it is returned as the
/// summary with no remaining description.
///
/// Otherwise, the first sentence is extracted as the summary. A sentence
/// ends at `. ` or `.\n`. The remainder becomes the description.
pub(crate) fn split_description(text: &str) -> (String, Option<String>) {
    let text = text.trim();

    // Find the first sentence boundary.
    // Look for ". " or ".\n" — a period followed by whitespace.
    for (i, _) in text.match_indices('.') {
        let after = i + 1;
        if after >= text.len() {
            // Period at end of string — the whole text is one sentence.
            break;
        }

        let next_byte = text.as_bytes()[after];
        if next_byte == b'\n' {
            // Period followed by newline is always a sentence boundary.
        } else if next_byte == b' ' {
            // Period followed by space: only split if the next non-space
            // character is uppercase (heuristic to skip abbreviations
            // like "e.g. foo").
            let rest_after_space = text[after..].trim_start();
            if rest_after_space.is_empty()
                || !rest_after_space
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
            {
                continue;
            }
        } else {
            continue;
        }

        {
            let summary = text[..=i].trim().to_owned();
            let rest = text[after..].trim();

            if rest.is_empty() {
                return (summary, None);
            }

            return (summary, Some(rest.to_owned()));
        }
    }

    // No sentence boundary found — take the first line.
    if let Some(nl) = text.find('\n') {
        let summary = text[..nl].trim().to_owned();
        let rest = text[nl..].trim();

        if rest.is_empty() {
            return (summary, None);
        }

        return (summary, Some(rest.to_owned()));
    }

    // Single long line, no period — return as-is.
    (text.to_owned(), None)
}

/// Fill in configured default values for missing parameters.
///
/// LLMs commonly omit parameters that have a `default` in the JSON schema,
/// even when those parameters are marked `required`. This function patches
/// the arguments map before validation so that such omissions don't cause
/// spurious "missing argument" errors and unnecessary LLM retries.
fn apply_parameter_defaults(
    arguments: &mut Map<String, Value>,
    parameters: &IndexMap<String, ToolParameterConfig>,
) {
    for (name, cfg) in parameters {
        if !arguments.contains_key(name) {
            if let Some(default) = &cfg.default {
                arguments.insert(name.clone(), default.clone());
            }
            continue;
        }

        // Recurse into object fields.
        if let Some(obj) = arguments.get_mut(name).and_then(Value::as_object_mut)
            && !cfg.properties.is_empty()
        {
            apply_parameter_defaults(obj, &cfg.properties);
        }

        // Recurse into array elements.
        if let Some(items) = &cfg.items
            && !items.properties.is_empty()
            && let Some(arr) = arguments.get_mut(name).and_then(Value::as_array_mut)
        {
            for elem in arr.iter_mut() {
                if let Some(obj) = elem.as_object_mut() {
                    apply_parameter_defaults(obj, &items.properties);
                }
            }
        }
    }
}

fn validate_tool_arguments(
    arguments: &Map<String, Value>,
    parameters: &IndexMap<String, ToolParameterConfig>,
) -> Result<(), ToolError> {
    let unknown = arguments
        .keys()
        .filter(|k| !parameters.contains_key(*k))
        .cloned()
        .collect::<Vec<_>>();

    let mut missing = vec![];
    for (name, cfg) in parameters {
        if cfg.required && !arguments.contains_key(name) {
            missing.push(name.to_owned());
        }
    }

    if !missing.is_empty() || !unknown.is_empty() {
        return Err(ToolError::Arguments { missing, unknown });
    }

    // Recurse into nested structures.
    for (name, cfg) in parameters {
        let Some(value) = arguments.get(name) else {
            continue;
        };

        // Object parameters with properties: validate the object fields.
        if let Some(obj) = value.as_object()
            && !cfg.properties.is_empty()
        {
            validate_tool_arguments(obj, &cfg.properties)?;
        }

        // Array parameters with items that have properties: validate each
        // element.
        if let Some(items) = &cfg.items
            && !items.properties.is_empty()
            && let Some(arr) = value.as_array()
        {
            for element in arr {
                if let Some(obj) = element.as_object() {
                    validate_tool_arguments(obj, &items.properties)?;
                }
            }
        }
    }

    Ok(())
}

/// Resolved tool definitions and their on-demand documentation.
pub struct ResolvedTools {
    /// Tool definitions sent to the LLM provider.
    pub definitions: Vec<ToolDefinition>,

    /// Per-tool documentation for `describe_tools`, keyed by tool name.
    pub docs: IndexMap<String, ToolDocs>,
}

pub async fn tool_definitions(
    configs: impl Iterator<Item = (&str, ToolConfigWithDefaults)>,
    mcp_client: &jp_mcp::Client,
) -> Result<ResolvedTools, ToolError> {
    let mut definitions = vec![];
    let mut docs = IndexMap::new();

    for (name, config) in configs {
        // Skip disabled tools.
        if !config.enable() {
            continue;
        }

        let (definition, tool_docs) = resolve_tool(name, &config, mcp_client).await?;

        if !tool_docs.is_empty() {
            docs.insert(name.to_owned(), tool_docs);
        }

        definitions.push(definition);
    }

    Ok(ResolvedTools { definitions, docs })
}

/// Resolve a single tool definition and its documentation.
async fn resolve_tool(
    name: &str,
    config: &ToolConfigWithDefaults,
    mcp_client: &jp_mcp::Client,
) -> Result<(ToolDefinition, ToolDocs), ToolError> {
    match config.source() {
        ToolSource::Local { .. } | ToolSource::Builtin { .. } => {
            // For local/builtin tools, docs come from config fields.
            let definition = ToolDefinition::new(
                name,
                config.source(),
                config.summary().map(str::to_owned),
                config.parameters().clone(),
                mcp_client,
            )
            .await?;

            let tool_docs = docs_from_config(config);
            Ok((definition, tool_docs))
        }
        ToolSource::Mcp { .. } => {
            // For MCP tools, resolve against the server, then build docs
            // from config overrides + auto-split of MCP descriptions.
            resolve_mcp_tool(name, config, mcp_client).await
        }
    }
}

/// Build `ToolDocs` from config fields (local/builtin tools).
fn docs_from_config(config: &ToolConfigWithDefaults) -> ToolDocs {
    let summary = config.summary().map(str::to_owned);
    let description = config.description().map(str::to_owned);
    let examples = config.examples().map(str::to_owned);

    let parameters = config
        .parameters()
        .iter()
        .filter_map(|(param_name, param_cfg)| {
            let summary = param_cfg
                .summary
                .as_deref()
                .or(param_cfg.description.as_deref())
                .map(str::to_owned);
            let desc = param_cfg.description.as_deref().map(str::to_owned);
            let ex = param_cfg.examples.as_deref().map(str::to_owned);

            if summary.is_none() && desc.is_none() && ex.is_none() {
                return None;
            }

            Some((param_name.to_owned(), ParameterDocs {
                summary,
                description: desc,
                examples: ex,
            }))
        })
        .collect();

    ToolDocs {
        summary,
        description,
        examples,
        parameters,
    }
}

/// Resolve an MCP tool: build definition + docs with auto-split heuristic.
async fn resolve_mcp_tool(
    name: &str,
    config: &ToolConfigWithDefaults,
    mcp_client: &jp_mcp::Client,
) -> Result<(ToolDefinition, ToolDocs), ToolError> {
    let has_user_summary = config.summary().is_some();

    let definition = ToolDefinition::new(
        name,
        config.source(),
        config.summary().map(str::to_owned),
        config.parameters().clone(),
        mcp_client,
    )
    .await?;

    // Build docs. If the user provided summary/description/examples in
    // config, use those. Otherwise, auto-split the resolved MCP description.
    let (summary, description) = if has_user_summary {
        // User provided explicit summary — use config fields as-is.
        (
            config.summary().map(str::to_owned),
            config.description().map(str::to_owned),
        )
    } else if let Some(resolved) = &definition.description {
        // No user summary — auto-split the MCP description.
        let (s, d) = split_description(resolved);
        (Some(s), d)
    } else {
        (None, None)
    };

    let examples = config.examples().map(str::to_owned);

    // Build parameter docs. For each parameter, check if the user
    // provided an override. If not, auto-split the MCP description.
    let parameters = definition
        .parameters
        .iter()
        .filter_map(|(param_name, param_cfg)| {
            let user_override = config.parameters().get(param_name);
            let has_user_param_summary = user_override.and_then(|o| o.summary.as_ref()).is_some();

            let (summary, desc) = if has_user_param_summary {
                // User provided explicit summary for this parameter.
                let summary = user_override
                    .and_then(|o| o.summary.as_deref())
                    .or(user_override.and_then(|o| o.description.as_deref()))
                    .map(str::to_owned);
                let desc = user_override
                    .and_then(|o| o.description.as_deref())
                    .map(str::to_owned);
                (summary, desc)
            } else if let Some(resolved) = &param_cfg.description {
                // Auto-split the resolved (possibly MCP) description.
                let (s, d) = split_description(resolved);
                (Some(s), d)
            } else {
                (None, None)
            };

            let ex = user_override
                .and_then(|o| o.examples.as_deref())
                .map(str::to_owned);

            if summary.is_none() && desc.is_none() && ex.is_none() {
                return None;
            }

            Some((param_name.to_owned(), ParameterDocs {
                summary,
                description: desc,
                examples: ex,
            }))
        })
        .collect();

    // The definition's description should be the summary (short) for the
    // provider API. Replace it if we auto-split.
    let mut definition = definition;
    if !has_user_summary && let Some(ref s) = summary {
        definition.description = Some(s.clone());
    }

    let tool_docs = ToolDocs {
        summary,
        description,
        examples,
        parameters,
    };

    Ok((definition, tool_docs))
}

fn local_tool_definition(
    name: String,
    description: Option<String>,
    parameters: IndexMap<String, ToolParameterConfig>,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        parameters,
    }
}

#[expect(clippy::too_many_lines)]
async fn mcp_tool_definition(
    server: Option<&String>,
    name: &str,
    source_name: Option<&str>,
    mut description: Option<String>,
    parameters: IndexMap<String, ToolParameterConfig>,
    mcp_client: &jp_mcp::Client,
) -> Result<ToolDefinition, ToolError> {
    let mcp_tool = {
        trace!(?server, tool = %name, "Fetching tool from MCP server");

        let server_id = server.as_ref().map(|s| McpServerId::new(s.to_owned()));
        mcp_client
            .get_tool(
                &McpToolId::new(source_name.unwrap_or(name)),
                server_id.as_ref(),
            )
            .await
            .map_err(ToolError::McpGetToolError)
    }?;

    match (description.as_mut(), mcp_tool.description) {
        (None, Some(mcp)) => {
            description = Some(mcp.to_string());
        }
        // TODO: should use `minijinja` instead.
        (Some(desc), Some(mcp)) => *desc = desc.replace("{{description}}", mcp.as_ref()),
        (Some(_) | None, None) => {}
    }

    let schema = mcp_tool.input_schema.as_ref().clone();
    let required_properties = schema
        .get("required")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();

    let mut params = IndexMap::new();
    for (name, opts) in schema
        .get("properties")
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
    {
        let override_cfg = parameters.get(name.as_str());

        let kind = match override_cfg.map(|v| v.kind.clone()) {
            // Use `override` type if present.
            Some(kind) => kind,
            // Or use the type from the schema.
            None => match opts.get("type").unwrap_or(&Value::Null) {
                Value::String(v) => OneOrManyTypes::One(v.to_owned()),
                Value::Array(v) => OneOrManyTypes::Many(
                    v.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                ),
                value => {
                    if value.is_null()
                        && let Some(any) = opts
                            .get("anyOf")
                            .and_then(Value::as_array)
                            .map(|v| {
                                v.iter()
                                    .filter_map(|v| {
                                        v.get("type").and_then(Value::as_str).map(str::to_owned)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .filter(|v| !v.is_empty())
                    {
                        OneOrManyTypes::Many(any)
                    } else {
                        return Err(ToolError::InvalidType {
                            key: name.to_owned(),
                            value: value.to_owned(),
                            need: vec!["string", "array"],
                        });
                    }
                }
            },
        };

        let default = override_cfg
            .and_then(|v| v.default.clone())
            .or_else(|| opts.get("default").cloned());

        let mut description = override_cfg.and_then(|v| v.description.clone());
        match (
            description.as_mut(),
            opts.get("description").and_then(Value::as_str),
        ) {
            (None, Some(mcp)) => {
                description = Some(mcp.to_string());
            }
            // TODO: should use `minijinja` instead.
            (Some(desc), Some(mcp)) => *desc = desc.replace("{{description}}", mcp.as_ref()),
            (Some(_) | None, None) => {}
        }

        let mut enumeration: Vec<Value> = override_cfg
            .map(|v| v.enumeration.clone())
            .into_iter()
            .flatten()
            .collect();

        if enumeration.is_empty() {
            enumeration = opts
                .get("enum")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .cloned()
                .collect();
        }

        // An MCP tool's parameter `requiredness` can be switched from `false`
        // to `true`, but not the other way around. This is because allowing
        // this could break the contract with the external tool's expectations.
        let required = required_properties.iter().any(|p| p == name);
        let required = match (required, override_cfg.map(|v| v.required)) {
            (v, None) => v,
            (true, _) => true,
            (false, Some(cfg)) => cfg,
        };

        params.insert(name.to_owned(), ToolParameterConfig {
            kind,
            default,
            required,
            summary: None,
            description,
            examples: None,
            enumeration,
            items: opts.get("items").and_then(|v| v.as_object()).and_then(|v| {
                Some(Box::new(ToolParameterConfig {
                    kind: match v.get("type")? {
                        Value::String(v) => OneOrManyTypes::One(v.to_owned()),
                        Value::Array(v) => OneOrManyTypes::Many(
                            v.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect(),
                        ),
                        _ => return None,
                    },
                    default: None,
                    required: false,
                    summary: None,
                    description: None,
                    examples: None,
                    enumeration: vec![],
                    items: None,
                    properties: IndexMap::default(),
                }))
            }),
            properties: IndexMap::default(),
        });
    }

    Ok(ToolDefinition {
        name: name.to_owned(),
        description,
        parameters: params,
    })
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
