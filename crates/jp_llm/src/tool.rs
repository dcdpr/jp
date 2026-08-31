//! Tool call utilities.

pub mod builtin;
pub mod executor;
pub mod json_schema;

use std::{ffi::OsStr, process::Stdio, sync::Arc};

pub use builtin::BuiltinTool;
use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::{
    conversation::tool::{CommandConfig, ToolConfigWithDefaults, ToolSource},
    types::command::shell_command_line,
};
use jp_conversation::event::ToolCallResponse;
use jp_mcp::{
    RawContent, ResourceContents,
    id::{McpServerId, McpToolId},
};
use jp_tool::{Action, Outcome, Question};
use json_schema::{Node, merge_description};
use minijinja::{Environment, ErrorKind as MinijinjaErrorKind, value::ValueKind};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};

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
#[derive(Debug, Clone, Default)]
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

    /// The short description used for the tool schema sent to the LLM.
    ///
    /// Returns `summary` if set, otherwise falls back to `description`.
    #[must_use]
    pub fn schema_description(&self) -> Option<&str> {
        self.summary.as_deref().or(self.description.as_deref())
    }

    /// Build `ToolDocs` from a tool's configuration.
    #[must_use]
    pub fn from_config(config: &ToolConfigWithDefaults) -> Self {
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

        Self {
            summary,
            description,
            examples,
            parameters,
        }
    }
}

/// The outcome of a tool execution.
///
/// This type represents the possible results of executing a tool's underlying
/// command or MCP call, without any interactive prompts.
/// The caller is responsible for:
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
    ///
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
    /// response.
    /// The caller should typically handle `NeedsInput` specially rather than
    /// converting it directly to a response.
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

    /// Returns `true` if this is a `Completed` outcome with a successful
    /// result.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Completed { result: Ok(_), .. })
    }
}

/// Result of running a tool command.
///
/// This is the single parsing point for all tool command output.
/// Both tool execution and argument formatting go through this type, ensuring
/// consistent handling of `Outcome` variants (including error traces).
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
    /// Falls back to treating stdout as plain text.
    /// The `success` flag indicates the process exit status.
    RawOutput {
        /// Raw stdout content.
        stdout: String,

        /// Raw stderr content.
        stderr: String,

        /// Whether the process exited successfully.
        success: bool,
    },

    /// Tool emitted a well-formed `needs_input` whose question id is invalid
    /// (empty, or contains a `.`, which is reserved as the inquiry-id
    /// separator).
    ///
    /// Surfaced as a tool-level error so the malformed inquiry is dropped
    /// before any inquiry event is constructed.
    InvalidInquiry {
        /// The offending question id, for the diagnostic trace.
        question_id: String,
    },

    /// Tool emitted a payload shaped like a `needs_input` outcome (top-level
    /// `"type": "needs_input"`) that failed to deserialize for a reason other
    /// than an invalid question id: a field with the wrong shape, a missing
    /// field, or a local-tool binary emitting an older wire protocol than this
    /// build parses.
    ///
    /// Surfaced as a tool-level error rather than [`Self::RawOutput`] so a
    /// protocol mismatch is loud, instead of silently handing the raw JSON to
    /// the model as tool output.
    MalformedInquiry {
        /// The deserialization error, for the diagnostic trace and the
        /// model-facing message.
        detail: String,
    },
}

impl CommandResult {
    /// Format a transient error message including trace details.
    ///
    /// If the trace is empty, returns just the message.
    /// Otherwise appends the trace entries so the LLM (or user) can see the
    /// root cause.
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
            Self::InvalidInquiry { question_id } => {
                error!(
                    tool = name,
                    question_id = %question_id,
                    "tool produced an invalid inquiry: question id must be non-empty and must not \
                     contain '.'"
                );
                Err(
                    "tool produced an invalid inquiry: question id must be non-empty and must not \
                     contain '.'"
                        .to_owned(),
                )
            }
            Self::MalformedInquiry { detail } => {
                error!(
                    tool = name,
                    %detail,
                    "tool produced a malformed inquiry that could not be parsed"
                );
                Err(format!(
                    "tool '{name}' produced a malformed inquiry that could not be parsed: {detail}"
                ))
            }
            Self::NeedsInput(_) => {
                unreachable!("NeedsInput should be handled by the caller")
            }
        }
    }
}

/// Identity of a tool invocation, used to tag stderr lines forwarded to
/// tracing.
///
/// Pass `None` to disable stderr forwarding (e.g. for argument-formatting
/// invocations where stderr is not meaningful to the user).
#[derive(Debug, Clone, Copy)]
pub struct ToolTrace<'a> {
    pub id: &'a str,
    pub name: &'a str,
}

/// Custom minijinja formatter used by [`run_tool_command`].
///
/// Scalars (strings, numbers, booleans) render raw — a template like
/// `{{tool.arguments.title}}` produces the bare string, not a JSON-quoted one.
/// Composites (sequences, maps, other iterables) serialize as JSON, so
/// `{{tool}}` and `{{context}}` produce valid JSON blobs without needing an
/// explicit `| tojson` filter at every call site.
/// `null`/undefined render as the literal `null`, matching the JSON convention
/// used by tool authors.
///
/// Safe strings (e.g. the output of the `tojson` filter) pass through unchanged
/// so explicit opt-in JSON rendering continues to work.
fn format_tool_template_value(
    out: &mut minijinja::Output<'_>,
    _state: &minijinja::State<'_, '_>,
    value: &minijinja::value::Value,
) -> Result<(), minijinja::Error> {
    if value.is_safe() {
        return write!(out, "{value}").map_err(Into::into);
    }

    match value.kind() {
        ValueKind::None | ValueKind::Undefined => write!(out, "null").map_err(Into::into),
        ValueKind::String | ValueKind::Bool | ValueKind::Number => {
            write!(out, "{value}").map_err(Into::into)
        }
        // Composites serialize as JSON so tool authors don't have to remember
        // `| tojson` for every `{{tool}}` / `{{context}}` interpolation.
        _ => {
            let json = serde_json::to_string(value).map_err(|error| {
                minijinja::Error::new(
                    MinijinjaErrorKind::BadSerialization,
                    "failed to serialize value as JSON",
                )
                .with_source(error)
            })?;
            out.write_str(&json).map_err(Into::into)
        }
    }
}

/// Run a tool command asynchronously with cancellation support.
///
/// This is the **single entry point** for running tool commands (both execution
/// and argument formatting).
/// It handles:
///
/// 1. Template rendering via [`minijinja`]
/// 2. Process spawning via Tokio's [`Command`]
/// 3. Cancellation via [`CancellationToken`]
/// 4. Parsing stdout as [`jp_tool::Outcome`]
/// 5. Forwarding the child's stderr to tracing (when `trace_as` is `Some`)
///
/// # Panics
///
/// Panics if tokio fails to attach the piped stdout/stderr handles to the
/// spawned child.
/// Both are requested via `Stdio::piped()`, so this is not expected to happen
/// in practice.
pub async fn run_tool_command(
    command: CommandConfig,
    ctx: Value,
    root: &Utf8Path,
    cancellation_token: CancellationToken,
    trace_as: Option<ToolTrace<'_>>,
) -> Result<CommandResult, ToolError> {
    let CommandConfig {
        program,
        args,
        shell,
    } = command;

    let mut env = Environment::new();
    env.set_formatter(format_tool_template_value);
    let tmpl = Arc::new(env);

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
        // `program` is shell syntax and used verbatim; `args` are shell-quoted
        // so multi-word arguments keep their boundaries.
        let shell_cmd = shell_command_line(&program, &args);

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&shell_cmd);
        cmd
    } else {
        let mut cmd = Command::new(&program);
        cmd.args(&args);
        cmd
    };

    // Isolate the child from JP's process group so terminal signals
    // (Ctrl+C / SIGINT) don't kill it. JP manages tool lifecycle via
    // the cancellation token, not Unix signals.
    #[cfg(unix)]
    cmd.process_group(0);

    // Ensure the child is killed when the tokio task is aborted on
    // cancellation. Without this the process would be orphaned.
    cmd.kill_on_drop(true);

    let mut child = cmd
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

    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let run = async {
        tokio::try_join!(
            read_all(stdout),
            forward_stderr(stderr, trace_as),
            child.wait(),
        )
    };

    tokio::select! {
        biased;
        () = cancellation_token.cancelled() => Ok(CommandResult::Cancelled),
        result = run => Ok(match result {
            Ok((stdout, stderr, status)) => {
                parse_command_output(&stdout, &stderr, status.success())
            }
            Err(error) => CommandResult::RawOutput {
                stdout: String::new(),
                stderr: error.to_string(),
                success: false,
            },
        }),
    }
}

/// Drain a child pipe into a byte buffer.
async fn read_all(mut pipe: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Drain a child's stderr into a byte buffer, optionally forwarding each line
/// to tracing as it arrives.
///
/// Uses byte-level line reading so non-UTF-8 stderr doesn't terminate the
/// forwarder.
async fn forward_stderr(
    pipe: impl tokio::io::AsyncRead + Unpin,
    trace_as: Option<ToolTrace<'_>>,
) -> std::io::Result<Vec<u8>> {
    let mut reader = BufReader::new(pipe);
    let mut all = Vec::new();
    let mut line = Vec::new();

    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            break;
        }

        if let Some(ToolTrace { id, name }) = trace_as {
            let text = String::from_utf8_lossy(&line);
            let trimmed = text.trim_end_matches(['\n', '\r']);
            if !trimmed.is_empty() {
                trace!(target: "tool::stderr", tool_id = id, tool_name = name, "{trimmed}");
            }
        }

        all.extend_from_slice(&line);
    }

    Ok(all)
}

/// Parse raw command output into a [`CommandResult`].
///
/// Tries to deserialize stdout as [`jp_tool::Outcome`].
/// If that fails, falls back to [`CommandResult::RawOutput`].
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
        // A payload shaped like a `needs_input` outcome that fails to
        // deserialize must become a tool-level error, not `RawOutput`:
        // silently handing the raw JSON to the model hides the failure (a
        // stale local-tool binary emitting an older wire shape than this build
        // parses, an invalid question id, a missing field) and leaves the
        // model to invent an explanation. Output that is not an `Outcome` at
        // all stays `RawOutput`.
        Err(error) => {
            let value = serde_json::from_str::<Value>(&stdout_str).ok();
            let is_needs_input = value
                .as_ref()
                .and_then(|v| v.get("type"))
                .and_then(Value::as_str)
                == Some("needs_input");

            if !is_needs_input {
                return CommandResult::RawOutput {
                    stdout: stdout_str.into_owned(),
                    stderr: String::from_utf8_lossy(stderr).into_owned(),
                    success,
                };
            }

            let question_id = value
                .as_ref()
                .and_then(|v| v.get("question"))
                .and_then(|q| q.get("id"))
                .and_then(Value::as_str);

            match question_id {
                // The id itself is the problem: empty, or containing the `.`
                // reserved as the inquiry-id separator (`QuestionId` rejects
                // both).
                Some(id) if id.is_empty() || id.contains('.') => CommandResult::InvalidInquiry {
                    question_id: id.to_owned(),
                },
                // Some other field failed to parse (wrong shape, missing
                // field, protocol skew).
                _ => CommandResult::MalformedInquiry {
                    detail: error.to_string(),
                },
            }
        }
    }
}

/// Identity of the conversation an invocation belongs to.
///
/// Surfaced to local tools through the rendered template `context` (as
/// `context.workspace_id` and `context.conversation_id`) so a tool can scope
/// any state it persists to the originating workspace and conversation.
#[derive(Debug, Clone, Default)]
pub struct InvocationContext {
    pub workspace_id: String,
    pub conversation_id: String,
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
    pub docs: ToolDocs,

    /// JSON Schema for the tool's arguments, as its source declared it, with
    /// configuration overrides applied.
    ///
    /// Adapting this to what a given API accepts belongs to that provider.
    pub parameters: Value,
}

impl ToolDefinition {
    /// Coerce JSON-encoded argument strings to non-string schema types.
    ///
    /// Strings stay unchanged when the schema accepts strings or their contents
    /// do not parse to a declared type.
    pub fn coerce_arguments(&self, arguments: &mut Map<String, Value>) {
        coerce_arguments_to_schema(arguments, &self.parameters);
    }

    /// Execute the tool without any interactive prompts.
    ///
    /// This is a pure execution method that runs the tool's underlying command
    /// or MCP call and returns an [`ExecutionOutcome`].
    /// All interactive decisions (permission prompts, result editing, question
    /// handling) are the caller's responsibility.
    ///
    /// # Arguments
    ///
    /// - `id` - The tool call ID for correlation with the request
    /// - `arguments` - The tool arguments (caller is responsible for any
    ///   pre-processing)
    /// - `answers` - Pre-provided answers to tool questions (from previous
    ///   `NeedsInput`)
    /// - `config` - Tool configuration
    /// - `mcp_client` - MCP client for MCP tool execution
    /// - `root` - Working directory for local tool execution
    /// - `cancellation_token` - Token to cancel long-running execution
    /// - `builtin_executors` - Registry of builtin tools
    ///
    /// # Returns
    ///
    /// - [`ExecutionOutcome::Completed`] - Tool finished (check inner `Result`
    ///   for success/error)
    /// - [`ExecutionOutcome::NeedsInput`] - Tool needs user input to continue
    /// - [`ExecutionOutcome::Cancelled`] - Execution was cancelled via the
    ///   token
    ///
    /// # Errors
    ///
    /// Returns [`ToolError`] for infrastructure errors (spawn failure, missing
    /// command, etc.).
    /// Tool-level errors (command returned non-zero) are returned as
    /// `Ok(ExecutionOutcome::Completed { result: Err(...) })`.
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
    #[expect(clippy::too_many_arguments)]
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
        access: Option<&jp_tool::AccessPolicy>,
        invocation: &InvocationContext,
    ) -> Result<ExecutionOutcome, ToolError> {
        let mut arguments = arguments;
        if let Some(arguments) = arguments.as_object_mut() {
            self.coerce_arguments(arguments);
        }
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
                    access,
                    invocation,
                )
                .await
            }
            ToolSource::Mcp { server, tool } => {
                self.execute_mcp(
                    id,
                    arguments,
                    mcp_client,
                    server,
                    tool.as_deref(),
                    cancellation_token,
                )
                .await
            }
            ToolSource::Builtin { tool } => {
                self.execute_builtin(id, &arguments, answers, tool.as_deref(), builtin_executors)
                    .await
            }
        }
    }

    /// Execute a local tool and return the outcome.
    ///
    /// This is the pure execution path for local tools.
    /// It validates arguments, runs the command, and converts the result to an
    /// `ExecutionOutcome`.
    async fn execute_local(
        &self,
        id: String,
        mut arguments: Value,
        answers: &IndexMap<String, Value>,
        config: &ToolConfigWithDefaults,
        tool: Option<&str>,
        root: &Utf8Path,
        cancellation_token: CancellationToken,
        access: Option<&jp_tool::AccessPolicy>,
        invocation: &InvocationContext,
    ) -> Result<ExecutionOutcome, ToolError> {
        let name = tool.unwrap_or(&self.name);

        // Apply configured defaults for missing parameters, then validate.
        if let Some(args) = arguments.as_object_mut() {
            apply_parameter_defaults(args, &self.parameters);

            if let Err(error) = validate_tool_arguments(args, &self.parameters) {
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
                "options": config.options(),
            },
            "context": {
                "action": Action::Run,
                "root": root.as_str(),
                "access": access,
                "workspace_id": &invocation.workspace_id,
                "conversation_id": &invocation.conversation_id,
            },
        });

        let Some(command) = config.command() else {
            return Err(ToolError::MissingCommand);
        };

        let trace_as = ToolTrace { id: &id, name };

        match run_tool_command(command, ctx, root, cancellation_token, Some(trace_as)).await? {
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
    /// This is the pure execution path for MCP tools.
    /// It calls the MCP server and converts the result to an
    /// `ExecutionOutcome`.
    async fn execute_mcp(
        &self,
        id: String,
        arguments: Value,
        mcp_client: &jp_mcp::Client,
        server: &str,
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
                        RawContent::Image(_) | RawContent::Audio(_) | RawContent::ResourceLink(_) => None,
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
    ///
    /// `source_name` is the implementation named by `source =
    /// "builtin.<name>"`, which the registry is keyed on.
    /// When absent, the implementation shares the tool's own name.
    async fn execute_builtin(
        &self,
        id: String,
        arguments: &Value,
        answers: &IndexMap<String, Value>,
        source_name: Option<&str>,
        builtin_executors: &builtin::BuiltinExecutors,
    ) -> Result<ExecutionOutcome, ToolError> {
        let name = source_name.unwrap_or(&self.name);
        let executor = builtin_executors
            .get(name)
            .ok_or_else(|| ToolError::NotFound {
                name: name.to_owned(),
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

    /// Return the JSON Schema for the tool's parameters.
    #[must_use]
    pub fn to_parameters_schema(&self) -> Value {
        self.parameters.clone()
    }
}

/// Split a description string into a short summary and remaining detail.
///
/// If the text is short (single line, ≤120 chars), it is returned as the
/// summary with no remaining description.
///
/// Otherwise, the first sentence is extracted as the summary.
/// A sentence ends at ` .  ` or `.\n`.
/// The remainder becomes the description.
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

/// Coerce JSON-encoded argument strings to the types the schema declares.
fn coerce_arguments_to_schema(arguments: &mut Map<String, Value>, schema: &Value) {
    coerce_object(arguments, &Node::root(schema));
}

fn coerce_object(arguments: &mut Map<String, Value>, node: &Node<'_>) {
    for (name, property) in node.properties() {
        if let Some(value) = arguments.get_mut(&name) {
            coerce_value(value, &property);
        }
    }
}

fn coerce_value(value: &mut Value, node: &Node<'_>) {
    if let Value::String(raw) = value
        && !node.types().iter().any(|type_| type_ == "string")
        && let Ok(parsed) = serde_json::from_str::<Value>(raw)
        && node.accepts(&parsed)
    {
        *value = parsed;
    }

    match value {
        Value::Object(arguments) => coerce_object(arguments, node),
        Value::Array(values) => {
            let Some(items) = node.items() else {
                return;
            };
            for value in values {
                coerce_value(value, &items);
            }
        }
        _ => {}
    }
}

/// Fill in configured default values for missing parameters.
///
/// LLMs commonly omit parameters that have a `default` in the JSON schema, even
/// when those parameters are marked `required`.
/// This function patches the arguments map before validation so that such
/// omissions don't cause spurious "missing argument" errors and unnecessary LLM
/// retries.
fn apply_parameter_defaults(arguments: &mut Map<String, Value>, schema: &Value) {
    apply_defaults_to(arguments, &Node::root(schema));
}

fn apply_defaults_to(arguments: &mut Map<String, Value>, node: &Node<'_>) {
    for (name, property) in node.properties() {
        if !arguments.contains_key(&name) {
            if let Some(default) = property.default() {
                let default = default.clone();
                arguments.insert(name, default);
            }
            continue;
        }

        // Recurse into object fields.
        if property.has_properties()
            && let Some(object) = arguments.get_mut(&name).and_then(Value::as_object_mut)
        {
            apply_defaults_to(object, &property);
        }

        // Recurse into array elements.
        if let Some(items) = property.items()
            && items.has_properties()
            && let Some(values) = arguments.get_mut(&name).and_then(Value::as_array_mut)
        {
            for value in values.iter_mut() {
                if let Some(object) = value.as_object_mut() {
                    apply_defaults_to(object, &items);
                }
            }
        }
    }
}

fn validate_tool_arguments(
    arguments: &Map<String, Value>,
    schema: &Value,
) -> Result<(), ToolError> {
    validate_arguments_against(arguments, &Node::root(schema))
}

fn validate_arguments_against(
    arguments: &Map<String, Value>,
    node: &Node<'_>,
) -> Result<(), ToolError> {
    let properties = node.properties();

    let unknown = arguments
        .keys()
        .filter(|name| !properties.iter().any(|(known, _)| known == *name))
        .cloned()
        .collect::<Vec<_>>();

    let missing = properties
        .iter()
        .filter(|(name, _)| node.is_required(name) && !arguments.contains_key(name))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    if !missing.is_empty() || !unknown.is_empty() {
        return Err(ToolError::Arguments { missing, unknown });
    }

    // Recurse into nested structures.
    for (name, property) in properties {
        let Some(value) = arguments.get(&name) else {
            continue;
        };

        if let Some(object) = value.as_object()
            && property.has_properties()
        {
            validate_arguments_against(object, &property)?;
        }

        if let Some(items) = property.items()
            && items.has_properties()
            && let Some(values) = value.as_array()
        {
            for value in values {
                if let Some(object) = value.as_object() {
                    validate_arguments_against(object, &items)?;
                }
            }
        }
    }

    Ok(())
}

/// Resolve all enabled tool definitions from config.
///
/// If `forced_tool` is provided (e.g. from `ToolChoice::Function`), that tool
/// is included even when it is disabled, preventing a mismatch between
/// `tool_choice` and the declared tools list that some providers (notably
/// Google/Gemini) reject outright.
///
/// A locked-off tool (`state = false`, `allow_toggle = never`) is the
/// exception: it is always dropped, even when named by `forced_tool`.
pub async fn tool_definitions(
    configs: impl Iterator<Item = (&str, ToolConfigWithDefaults)>,
    mcp_client: &jp_mcp::Client,
    forced_tool: Option<&str>,
) -> Result<Vec<ToolDefinition>, ToolError> {
    let mut definitions = Vec::new();

    for (name, config) in configs {
        let enable = config.effective_enable();
        let forced = forced_tool.is_some_and(|f| f == name);
        // Drop disabled tools, but keep a forced tool unless it is locked-off.
        if !enable.is_enabled() && (!forced || enable.is_locked()) {
            continue;
        }

        // Drop MCP-backed tools whose server failed to start while marked
        // optional. The server is absent from the running services map, and
        // we don't want to hand the LLM a tool it cannot invoke.
        if let ToolSource::Mcp { server, .. } = config.source() {
            let server_id = McpServerId::new(server);
            if !mcp_client.is_running(&server_id).await {
                warn!(
                    tool = name,
                    server = %server,
                    "Skipping MCP tool: backing server is not running."
                );
                continue;
            }
        }

        // A tool JP cannot describe to the provider is dropped rather than
        // failing the query, matching the unavailable-server case above. A tool
        // the caller named explicitly is the exception: silently omitting it
        // would leave `tool_choice` pointing at a tool the provider never saw.
        let definition = match resolve_tool(name, &config, mcp_client).await {
            Ok(definition) => definition,
            Err(error) if !forced => {
                warn!(
                    tool = name,
                    %error,
                    "Skipping tool: its parameter schema could not be resolved."
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        definitions.push(definition);
    }

    Ok(definitions)
}

/// Resolve a single tool definition and its documentation.
async fn resolve_tool(
    name: &str,
    config: &ToolConfigWithDefaults,
    mcp_client: &jp_mcp::Client,
) -> Result<ToolDefinition, ToolError> {
    let path = format!("conversation.tools.{name}.parameters");
    let definition = match config.source() {
        ToolSource::Local { .. } | ToolSource::Builtin { .. } => ToolDefinition {
            name: name.to_owned(),
            docs: ToolDocs::from_config(config),
            parameters: json_schema::from_config(&path, config.parameters())?,
        },
        ToolSource::Mcp { server, tool } => {
            resolve_mcp_tool(server, name, tool.as_deref(), config, mcp_client).await?
        }
    };

    json_schema::validate(&path, &definition.parameters)?;

    Ok(definition)
}

/// Resolve an MCP tool: fetch from server, merge config overrides, auto-split
/// descriptions into summary + detail.
async fn resolve_mcp_tool(
    server: &str,
    name: &str,
    source_name: Option<&str>,
    config: &ToolConfigWithDefaults,
    mcp_client: &jp_mcp::Client,
) -> Result<ToolDefinition, ToolError> {
    let mcp_tool = {
        trace!(server = %server, tool = %name, "Fetching tool from MCP server");

        let server_id = McpServerId::new(server);
        mcp_client
            .get_tool(&McpToolId::new(source_name.unwrap_or(name)), &server_id)
            .await
            .map_err(ToolError::McpGetToolError)
    }?;

    let user_overrides = config.parameters();

    // Merge tool-level description.
    let merged_description = merge_description(
        config.description().map(str::to_owned),
        mcp_tool.description.as_deref(),
    );

    // The server's document is the source of truth; configuration may narrow
    // it, and nothing else touches it.
    let source = Value::Object(mcp_tool.input_schema.as_ref().clone());
    let parameters = json_schema::with_overrides(
        &format!("conversation.tools.{name}.parameters"),
        &source,
        user_overrides,
    )?;

    // Build docs with auto-split heuristic.
    let has_user_summary = config.summary().is_some();

    let (summary, description) = if has_user_summary {
        // User provided explicit summary -- use config fields as-is.
        (
            config.summary().map(str::to_owned),
            config.description().map(str::to_owned),
        )
    } else if let Some(ref desc) = merged_description {
        let (s, d) = split_description(desc);
        (Some(s), d)
    } else {
        (None, None)
    };

    let examples = config.examples().map(str::to_owned);

    // Per-parameter docs: auto-split MCP descriptions when user didn't override.
    let param_docs = Node::root(&parameters)
        .properties()
        .into_iter()
        .filter_map(|(pname, pnode)| {
            let user_override = user_overrides.get(&pname);
            let has_user_param_summary = user_override.and_then(|o| o.summary.as_ref()).is_some();

            let (summary, desc) = if has_user_param_summary {
                let summary = user_override
                    .and_then(|o| o.summary.as_deref())
                    .or(user_override.and_then(|o| o.description.as_deref()))
                    .map(str::to_owned);
                let desc = user_override
                    .and_then(|o| o.description.as_deref())
                    .map(str::to_owned);
                (summary, desc)
            } else if let Some(resolved) = pnode.description() {
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

            Some((pname, ParameterDocs {
                summary,
                description: desc,
                examples: ex,
            }))
        })
        .collect();

    let docs = ToolDocs {
        summary,
        description,
        examples,
        parameters: param_docs,
    };

    Ok(ToolDefinition {
        name: name.to_owned(),
        docs,
        parameters,
    })
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
