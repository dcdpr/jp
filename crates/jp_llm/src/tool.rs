//! Tool call utilities.

pub mod builtin;
pub mod executor;
mod schema;

use std::{ffi::OsStr, process::Stdio, sync::Arc};

pub use builtin::BuiltinTool;
use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::{
    conversation::tool::{
        CommandConfig, OneOrManyTypes, ToolConfigWithDefaults, ToolParameterConfig, ToolSource,
    },
    types::command::shell_command_line,
};
use jp_conversation::event::ToolCallResponse;
use jp_mcp::{
    RawContent, ResourceContents,
    id::{McpServerId, McpToolId},
};
use jp_tool::{Action, Outcome, Question};
use minijinja::{Environment, ErrorKind as MinijinjaErrorKind, value::ValueKind};
pub use schema::ToolParameterSchema;
use schema::{
    format_types, parameter_accepts_value, types_match, validate_parameter_schema, validate_types,
};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, trace, warn};

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
        Err(_) => CommandResult::RawOutput {
            stdout: stdout_str.into_owned(),
            stderr: String::from_utf8_lossy(stderr).into_owned(),
            success,
        },
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
    pub parameters: IndexMap<String, ToolParameterSchema>,
}

impl ToolDefinition {
    /// Coerce JSON-encoded argument strings to non-string schema types.
    ///
    /// Strings stay unchanged when the schema accepts strings or their contents
    /// do not parse to a declared type.
    pub fn coerce_arguments(&self, arguments: &mut Map<String, Value>) {
        coerce_parameter_types(arguments, &self.parameters);
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
            ToolSource::Builtin { .. } => {
                self.execute_builtin(id, &arguments, answers, builtin_executors)
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

fn coerce_parameter_types(
    arguments: &mut Map<String, Value>,
    parameters: &IndexMap<String, ToolParameterSchema>,
) {
    for (name, config) in parameters {
        let Some(value) = arguments.get_mut(name) else {
            continue;
        };

        coerce_parameter_value(value, config);
    }
}

fn coerce_parameter_value(value: &mut Value, config: &ToolParameterSchema) {
    if let Value::String(raw) = value
        && !config.kind.has_type("string")
        && let Ok(parsed) = serde_json::from_str(raw)
        && parameter_accepts_value(&parsed, &config.kind)
    {
        *value = parsed;
    }

    match value {
        Value::Object(arguments) if !config.properties.is_empty() => {
            coerce_parameter_types(arguments, &config.properties);
        }
        Value::Array(values) => {
            let Some(items) = config.items.as_deref() else {
                return;
            };
            for value in values {
                coerce_parameter_value(value, items);
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
fn apply_parameter_defaults(
    arguments: &mut Map<String, Value>,
    parameters: &IndexMap<String, ToolParameterSchema>,
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
    parameters: &IndexMap<String, ToolParameterSchema>,
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

        let definition = resolve_tool(name, &config, mcp_client).await?;
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
    let definition = match config.source() {
        ToolSource::Local { .. } | ToolSource::Builtin { .. } => {
            let docs = ToolDocs::from_config(config);
            let parameters = config
                .parameters()
                .iter()
                .map(|(parameter_name, parameter)| {
                    resolve_config_parameter(
                        &format!("conversation.tools.{name}.parameters.{parameter_name}"),
                        parameter,
                    )
                    .map(|schema| (parameter_name.clone(), schema))
                })
                .collect::<Result<_, _>>()?;
            ToolDefinition {
                name: name.to_owned(),
                docs,
                parameters,
            }
        }
        ToolSource::Mcp { server, tool } => {
            resolve_mcp_tool(server, name, tool.as_deref(), config, mcp_client).await?
        }
    };

    for (parameter_name, parameter) in &definition.parameters {
        validate_parameter_schema(
            &format!("conversation.tools.{name}.parameters.{parameter_name}"),
            parameter,
        )?;
    }

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

    // Build parameters from MCP schema + user overrides.
    let schema = mcp_tool.input_schema.as_ref().clone();
    let required_properties: Vec<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect();

    let mut params = IndexMap::new();
    for (param_name, opts) in schema
        .get("properties")
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
    {
        let override_cfg = user_overrides.get(param_name.as_str());
        let required_in_mcp = required_properties.iter().any(|p| *p == param_name);
        let param = merge_mcp_parameter(
            &format!("conversation.tools.{name}.parameters.{param_name}"),
            opts,
            override_cfg,
            required_in_mcp,
        )?;
        params.insert(param_name.to_owned(), param);
    }

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
    let param_docs = params
        .iter()
        .filter_map(|(pname, pcfg)| {
            let user_override = user_overrides.get(pname);
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
            } else if let Some(resolved) = &pcfg.description {
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

            Some((pname.to_owned(), ParameterDocs {
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
        parameters: params,
    })
}

fn resolve_config_parameter(
    path: &str,
    config: &ToolParameterConfig,
) -> Result<ToolParameterSchema, ToolError> {
    let kind = config
        .kind
        .clone()
        .ok_or_else(|| ToolError::InvalidSchema {
            path: format!("{path}.type"),
            message: "local and built-in tool parameters must declare a type".to_owned(),
        })?;
    let items = config
        .items
        .as_deref()
        .map(|items| resolve_config_parameter(&format!("{path}.items"), items))
        .transpose()?
        .map(Box::new);
    let properties = config
        .properties
        .iter()
        .map(|(name, property)| {
            resolve_config_parameter(&format!("{path}.properties.{name}"), property)
                .map(|schema| (name.clone(), schema))
        })
        .collect::<Result<_, _>>()?;

    Ok(ToolParameterSchema {
        kind,
        default: config.default.clone(),
        required: config.required.unwrap_or(false),
        summary: config.summary.clone(),
        description: config.description.clone(),
        examples: config.examples.clone(),
        enumeration: config.enumeration.clone().unwrap_or_default(),
        items,
        properties,
    })
}

#[cfg(test)]
fn merge_mcp_param(
    name: &str,
    source: &Value,
    override_config: Option<&ToolParameterConfig>,
    required_in_mcp: bool,
) -> Result<ToolParameterSchema, ToolError> {
    merge_mcp_parameter(
        &format!("parameters.{name}"),
        source,
        override_config,
        required_in_mcp,
    )
}

fn merge_mcp_parameter(
    path: &str,
    source: &Value,
    override_config: Option<&ToolParameterConfig>,
    required_in_mcp: bool,
) -> Result<ToolParameterSchema, ToolError> {
    let schema = merge_mcp_schema_node(path, source, override_config, required_in_mcp)?;
    validate_parameter_schema(path, &schema)?;
    Ok(schema)
}

fn merge_mcp_schema_node(
    path: &str,
    source: &Value,
    override_config: Option<&ToolParameterConfig>,
    required_in_mcp: bool,
) -> Result<ToolParameterSchema, ToolError> {
    let source_kind = parse_schema_kind(path, source)?;
    let override_kind = override_config.and_then(|config| config.kind.as_ref());

    // The resolved schema keeps the server's type declaration, so a malformed
    // override type list would otherwise never be reported.
    if let Some(override_kind) = override_kind {
        validate_types(path, override_kind)?;
    }

    let kind = match (source_kind, override_kind) {
        (Some(source), Some(config)) if !types_match(&source, config) => {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.type"),
                message: format!(
                    "MCP declares {}, but the configuration declares {}",
                    format_types(&source),
                    format_types(config)
                ),
            });
        }
        (Some(source), _) => source,
        (None, Some(config)) => config.clone(),
        (None, None) => {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.type"),
                message: "schema does not declare a supported type".to_owned(),
            });
        }
    };

    let default = override_config
        .and_then(|config| config.default.clone())
        .or_else(|| source.get("default").cloned());
    let description = merge_description(
        override_config.and_then(|config| config.description.clone()),
        source.get("description").and_then(Value::as_str),
    );
    let enumeration =
        if let Some(enumeration) = override_config.and_then(|config| config.enumeration.clone()) {
            enumeration
        } else {
            parse_schema_enum(path, source)?
        };
    let required = required_in_mcp
        || override_config
            .and_then(|config| config.required)
            .unwrap_or(false);

    let items = merge_mcp_items(path, source, override_config)?;
    let properties = merge_mcp_properties(path, source, override_config)?;

    Ok(ToolParameterSchema {
        kind,
        default,
        required,
        summary: override_config.and_then(|config| config.summary.clone()),
        description,
        examples: override_config.and_then(|config| config.examples.clone()),
        enumeration,
        items,
        properties,
    })
}

fn merge_mcp_items(
    path: &str,
    source: &Value,
    override_config: Option<&ToolParameterConfig>,
) -> Result<Option<Box<ToolParameterSchema>>, ToolError> {
    match (
        source.get("items"),
        override_config.and_then(|config| config.items.as_deref()),
    ) {
        (Some(source_items), override_items) => merge_mcp_schema_node(
            &format!("{path}.items"),
            source_items,
            override_items,
            false,
        )
        .map(Box::new)
        .map(Some),
        (None, Some(override_items)) => {
            resolve_config_parameter(&format!("{path}.items"), override_items)
                .map(Box::new)
                .map(Some)
        }
        (None, None) => Ok(None),
    }
}

fn merge_mcp_properties(
    path: &str,
    source: &Value,
    override_config: Option<&ToolParameterConfig>,
) -> Result<IndexMap<String, ToolParameterSchema>, ToolError> {
    let source_required = source
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>();
    let mut properties = IndexMap::new();

    if let Some(source_properties) = source.get("properties").and_then(Value::as_object) {
        for (name, source_property) in source_properties {
            let override_property = override_config.and_then(|config| config.properties.get(name));
            let required = source_required.iter().any(|required| required == name);
            let property = merge_mcp_schema_node(
                &format!("{path}.properties.{name}"),
                source_property,
                override_property,
                required,
            )?;
            properties.insert(name.clone(), property);
        }
    }

    if let Some(override_config) = override_config {
        for (name, override_property) in &override_config.properties {
            if properties.contains_key(name) {
                continue;
            }
            let property =
                resolve_config_parameter(&format!("{path}.properties.{name}"), override_property)?;
            properties.insert(name.clone(), property);
        }
    }

    Ok(properties)
}

fn parse_schema_enum(path: &str, schema: &Value) -> Result<Vec<Value>, ToolError> {
    match schema.get("enum") {
        Some(Value::Array(values)) => Ok(values.clone()),
        Some(_) => Err(ToolError::InvalidSchema {
            path: format!("{path}.enum"),
            message: "enum must be an array".to_owned(),
        }),
        None => Ok(Vec::new()),
    }
}

fn parse_schema_kind(path: &str, schema: &Value) -> Result<Option<OneOrManyTypes>, ToolError> {
    if let Some(type_) = schema.get("type") {
        return match type_ {
            Value::String(type_) => Ok(Some(OneOrManyTypes::One(type_.clone()))),
            Value::Array(types) => {
                let types = types
                    .iter()
                    .map(|type_| {
                        type_
                            .as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| ToolError::InvalidSchema {
                                path: format!("{path}.type"),
                                message: "type arrays may contain only strings".to_owned(),
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(OneOrManyTypes::Many(types)))
            }
            _ => Err(ToolError::InvalidSchema {
                path: format!("{path}.type"),
                message: "type must be a string or an array of strings".to_owned(),
            }),
        };
    }

    let variants = schema.get("anyOf").and_then(Value::as_array);
    let types = variants
        .into_iter()
        .flatten()
        .filter_map(|variant| variant.get("type").and_then(Value::as_str))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if types.is_empty() {
        return Ok(None);
    }

    // A variant carrying no plain `type` (a `$ref` into `$defs`, say) cannot be
    // represented, so the resolved type set is narrower than the server's
    // contract. Rejecting instead would fail every query that enables the tool,
    // so surface it and continue.
    if variants.is_some_and(|variants| variants.len() > types.len()) {
        warn!(
            path,
            resolved = ?types,
            "Ignoring `anyOf` variants that declare no JSON type; the schema sent to the \
             provider is narrower than the MCP server's."
        );
    }

    Ok(Some(OneOrManyTypes::Many(types)))
}

/// Merge a user-provided description with an MCP server description.
///
/// If the user provided a description containing `{{description}}`, the MCP
/// description is substituted in.
/// If the user provided a description without the template, it takes
/// precedence.
/// If no user description exists, the MCP description is used as-is.
fn merge_description(user: Option<String>, mcp: Option<&str>) -> Option<String> {
    match (user, mcp) {
        (None, Some(mcp)) => Some(mcp.to_owned()),
        // TODO: should use `minijinja` instead of raw string replacement.
        (Some(desc), Some(mcp)) => Some(desc.replace("{{description}}", mcp)),
        (Some(desc), None) => Some(desc),
        (None, None) => None,
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
