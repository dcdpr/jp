//! JP tool execution and MCP serving.
//!
//! [`service::Service`] coordinates tool execution with the MCP Host through
//! private interaction channels.
//! [`http::Endpoint`] exposes that service over loopback Streamable HTTP.
//!
//! [`tool_definitions`] resolves the configured catalog.
//! [`execute`] runs one attempt of a local command, built-in implementation, or
//! upstream stdio MCP tool; the service handles input-driven re-execution and
//! delivery barriers.

pub mod builtin;
pub mod http;
pub mod json_schema;
pub mod service;
mod upstream;
use std::{convert::identity, ffi::OsStr, fmt, process::Stdio, sync::Arc};

pub use builtin::BuiltinTool;
use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::{
    conversation::tool::{CommandConfig, ToolConfigWithDefaults, ToolSource},
    types::command::shell_command_line,
};
use jp_tool::{
    AccessPolicy, Action, Error as ToolError, Outcome, ParameterDocs, Question, ToolDefinition,
    ToolDocs,
    definition::{apply_parameter_defaults, split_description, validate_tool_arguments},
    schema::{Node, merge_description},
};
use minijinja::{Environment, ErrorKind as MinijinjaErrorKind, value::ValueKind};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};
pub use upstream::text_result;
use upstream::{UpstreamResult, decode_result, replace_envelope};

use crate::{
    CallToolResult, Client,
    id::{McpServerId, McpToolId},
};

/// Build trusted execution context for local templates and upstream MCP
/// metadata.
pub(crate) fn tool_context(
    name: &str,
    arguments: &Value,
    answers: &IndexMap<String, Value>,
    config: &ToolConfigWithDefaults,
    root: &Utf8Path,
    action: &Action,
    access: Option<&AccessPolicy>,
    invocation: &InvocationContext,
) -> Value {
    json!({
        "tool": { "name":name, "arguments":arguments, "answers":answers, "options":config.options() },
        "context": { "action":action, "root":root.as_str(), "access":access,
            "workspace_id":invocation.workspace_id, "conversation_id":invocation.conversation_id }
    })
}

/// Read a tool's documentation out of its configuration.
fn tool_docs_from_config(config: &ToolConfigWithDefaults) -> ToolDocs {
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
        summary: config.summary().map(str::to_owned),
        description: config.description().map(str::to_owned),
        examples: config.examples().map(str::to_owned),
        parameters,
    }
}

/// The outcome of a tool execution.
///
/// This type represents the possible results of executing a tool's underlying
/// command or MCP call, without any interactive prompts.
/// The caller is responsible for:
///
/// 1. Handling permission prompts **before** calling [`execute()`].
/// 2. Handling [`ExecutionOutcome::NeedsInput`] by prompting the user or
///    assistant.
/// 3. Handling result editing **after** receiving the outcome.
///
/// # Example Flow
///
/// ```text
/// host                                     execute()
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
        /// Full upstream MCP result before the Host's compatibility projection.
        native: Option<CallToolResult>,
    },

    /// Tool needs additional input before it can complete.
    ///
    /// The caller should:
    ///
    /// 1. Present the question to the user (or delegate to the assistant)
    /// 2. Collect the answer
    /// 3. Call [`execute()`] again with the answer in `answers`
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

/// Receives a running tool's stderr lines as they arrive.
///
/// Called from the forwarder's read loop, so it must not block: the loop has to
/// keep draining or the child fills its pipe and the tool call never completes.
/// A consumer that falls behind drops rather than stalls.
pub type StderrSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Identity of a tool invocation, used to tag stderr lines forwarded to
/// tracing.
///
/// Pass `None` to disable stderr forwarding (e.g. for argument-formatting
/// invocations where stderr is not meaningful to the user).
#[derive(Clone)]
pub struct ToolTrace<'a> {
    pub id: &'a str,
    pub name: &'a str,

    /// Where to send each line for display, in addition to tracing.
    ///
    /// `None` when nothing is watching, which is the common case: tracing and
    /// the accumulated buffer are unaffected either way.
    pub stderr: Option<StderrSink>,
}

impl fmt::Debug for ToolTrace<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolTrace")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("stderr", &self.stderr.is_some())
            .finish()
    }
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
            error: Box::new(error),
        })?;

    let args = args
        .iter()
        .map(|s| tmpl.render_str(s, &ctx))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ToolError::TemplateError {
            data: args.join(" "),
            error: Box::new(error),
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

        if let Some(ToolTrace { id, name, stderr }) = &trace_as {
            let text = String::from_utf8_lossy(&line);
            let trimmed = text.trim_end_matches(['\n', '\r']);
            if !trimmed.is_empty() {
                trace!(target: "tool::stderr", tool_id = id, tool_name = name, "{trimmed}");

                if let Some(sink) = stderr {
                    sink(trimmed);
                }
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

/// Execute a tool without any interactive prompts.
///
/// Runs one attempt through the tool's command or MCP call and returns an
/// [`ExecutionOutcome`].
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
/// - [`ExecutionOutcome::Completed`] - Tool finished (check inner `Result` for
///   success/error)
/// - [`ExecutionOutcome::NeedsInput`] - Tool needs user input to continue
/// - [`ExecutionOutcome::Cancelled`] - Execution was cancelled via the token
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
///     match execute(&definition, id, args, &answers, ...).await? {
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
    definition: &ToolDefinition,
    id: String,
    arguments: Value,
    answers: &IndexMap<String, Value>,
    config: &ToolConfigWithDefaults,
    mcp_client: &Client,
    root: &Utf8Path,
    cancellation_token: CancellationToken,
    builtin_executors: &builtin::BuiltinExecutors,
    access: Option<&jp_tool::AccessPolicy>,
    invocation: &InvocationContext,
    stderr: Option<StderrSink>,
) -> Result<ExecutionOutcome, ToolError> {
    let mut arguments = arguments;
    if let Some(arguments) = arguments.as_object_mut() {
        definition.coerce_arguments(arguments);
    }
    info!(tool = %definition.name, arguments = ?arguments, "Executing tool.");

    match config.source() {
        ToolSource::Local { tool } => {
            execute_local(
                definition,
                id,
                arguments,
                answers,
                config,
                tool.as_deref(),
                root,
                cancellation_token,
                access,
                invocation,
                stderr,
            )
            .await
        }
        ToolSource::Mcp { server, tool } => {
            execute_mcp(
                definition,
                id,
                arguments,
                mcp_client,
                server,
                tool.as_deref(),
                answers,
                config,
                root,
                access,
                invocation,
                cancellation_token,
            )
            .await
        }
        ToolSource::Builtin { tool } => {
            execute_builtin(
                definition,
                id,
                &arguments,
                answers,
                tool.as_deref(),
                builtin_executors,
            )
            .await
        }
    }
}

/// Execute a local tool and return the outcome.
///
/// Runs one local command attempt.
/// It validates arguments, runs the command, and converts the result to an
/// `ExecutionOutcome`.
#[expect(clippy::too_many_arguments)]
async fn execute_local(
    definition: &ToolDefinition,
    id: String,
    mut arguments: Value,
    answers: &IndexMap<String, Value>,
    config: &ToolConfigWithDefaults,
    tool: Option<&str>,
    root: &Utf8Path,
    cancellation_token: CancellationToken,
    access: Option<&jp_tool::AccessPolicy>,
    invocation: &InvocationContext,
    stderr: Option<StderrSink>,
) -> Result<ExecutionOutcome, ToolError> {
    let name = tool.unwrap_or(&definition.name);

    // Apply configured defaults for missing parameters, then validate.
    if let Some(args) = arguments.as_object_mut() {
        apply_parameter_defaults(args, &definition.parameters);

        if let Err(error) = validate_tool_arguments(args, &definition.parameters) {
            return Ok(ExecutionOutcome::Completed {
                native: None,
                id,
                result: Err(format!(
                    "Invalid arguments: {error}\n\nYou can call `describe_tools(tools: \
                     [\"{name}\"])` to learn more about how to use the tool correctly."
                )),
            });
        }
    }

    let ctx = tool_context(
        name,
        &arguments,
        answers,
        config,
        root,
        &Action::Run,
        access,
        invocation,
    );

    let Some(command) = config.command() else {
        return Err(ToolError::MissingCommand);
    };

    let trace_as = ToolTrace {
        id: &id,
        name,
        stderr,
    };

    match run_tool_command(command, ctx, root, cancellation_token, Some(trace_as)).await? {
        CommandResult::Success(content) => Ok(ExecutionOutcome::Completed {
            native: None,
            id,
            result: Ok(content),
        }),
        CommandResult::NeedsInput(question) => Ok(ExecutionOutcome::NeedsInput { id, question }),
        CommandResult::Cancelled => Ok(ExecutionOutcome::Cancelled { id }),
        other => Ok(ExecutionOutcome::Completed {
            native: None,
            id,
            result: other.into_tool_result(name),
        }),
    }
}

/// Execute an MCP tool and return the outcome.
///
/// Runs one upstream MCP call.
/// It calls the MCP server and converts the result to an `ExecutionOutcome`.
#[expect(clippy::too_many_arguments)]
async fn execute_mcp(
    definition: &ToolDefinition,
    id: String,
    arguments: Value,
    mcp_client: &Client,
    server: &str,
    tool: Option<&str>,
    answers: &IndexMap<String, Value>,
    config: &ToolConfigWithDefaults,
    root: &Utf8Path,
    access: Option<&AccessPolicy>,
    invocation: &InvocationContext,
    cancellation_token: CancellationToken,
) -> Result<ExecutionOutcome, ToolError> {
    let name = tool.unwrap_or(&definition.name);

    let context = tool_context(
        name,
        &arguments,
        answers,
        config,
        root,
        &Action::Run,
        access,
        invocation,
    );
    let meta = Map::from_iter([
        ("computer.jp/tool".into(), context["tool"].clone()),
        ("computer.jp/context".into(), context["context"].clone()),
    ]);
    let call_future = mcp_client.call_tool(name, server, &arguments, Some(meta));

    tokio::select! {
        biased;
        () = cancellation_token.cancelled() => {
            info!(tool = %definition.name, "MCP tool call cancelled");
            Ok(ExecutionOutcome::Cancelled { id })
        }
        result = call_future => {
            let result = result
                .map_err(|error| ToolError::McpRunToolError(Box::new(error)))?;

            let result = match decode_result(result).map_err(ToolError::MalformedOutput)? {
                UpstreamResult::Outcome { outcome, response } => return Ok(match outcome {
                    Outcome::Success {content} => ExecutionOutcome::Completed {id, native:Some(replace_envelope(response, &content, false)), result:Ok(content)},
                    Outcome::NeedsInput {question} => ExecutionOutcome::NeedsInput {id, question},
                    Outcome::Error {message, trace, transient} => {
                        let text = if transient {
                            json!({"message":message, "trace":trace}).to_string()
                        } else {
                            text_result(&response).unwrap_or_else(identity)
                        };
                        let native = Some(replace_envelope(response, &text, true));
                        ExecutionOutcome::Completed {id, result:Err(text), native}
                    }
                }),
                UpstreamResult::Native(result) => result,
            };
            let text = text_result(&result);
            Ok(ExecutionOutcome::Completed { id, result: text, native: Some(result) })
        }
    }
}

/// Execute a builtin tool and return the outcome.
///
/// `source_name` is the implementation named by `source = "builtin.<name>"`,
/// which the registry is keyed on.
/// When absent, the implementation shares the tool's own name.
async fn execute_builtin(
    definition: &ToolDefinition,
    id: String,
    arguments: &Value,
    answers: &IndexMap<String, Value>,
    source_name: Option<&str>,
    builtin_executors: &builtin::BuiltinExecutors,
) -> Result<ExecutionOutcome, ToolError> {
    let name = source_name.unwrap_or(&definition.name);
    let executor = builtin_executors
        .get(name)
        .ok_or_else(|| ToolError::NotFound {
            name: name.to_owned(),
        })?;

    let outcome = executor.execute(arguments, answers).await;

    Ok(match outcome {
        Outcome::Success { content } => ExecutionOutcome::Completed {
            native: None,
            id,
            result: Ok(content),
        },
        Outcome::Error {
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
                native: None,
                id,
                result: Err(error_msg),
            }
        }
        Outcome::NeedsInput { question } => ExecutionOutcome::NeedsInput { id, question },
    })
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
    mcp_client: &Client,
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
    mcp_client: &Client,
) -> Result<ToolDefinition, ToolError> {
    let path = format!("conversation.tools.{name}.parameters");
    let definition = match config.source() {
        ToolSource::Local { .. } | ToolSource::Builtin { .. } => ToolDefinition {
            name: name.to_owned(),
            docs: tool_docs_from_config(config),
            parameters: json_schema::from_config(&path, config.parameters())?,
        },
        ToolSource::Mcp { server, tool } => {
            resolve_mcp_tool(server, name, tool.as_deref(), config, mcp_client).await?
        }
    };

    jp_tool::schema::validate(&path, &definition.parameters)?;

    Ok(definition)
}

/// Resolve an MCP tool: fetch from server, merge config overrides, auto-split
/// descriptions into summary + detail.
async fn resolve_mcp_tool(
    server: &str,
    name: &str,
    source_name: Option<&str>,
    config: &ToolConfigWithDefaults,
    mcp_client: &Client,
) -> Result<ToolDefinition, ToolError> {
    let mcp_tool = {
        trace!(server = %server, tool = %name, "Fetching tool from MCP server");

        let server_id = McpServerId::new(server);
        mcp_client
            .get_tool(&McpToolId::new(source_name.unwrap_or(name)), &server_id)
            .await
            .map_err(|error| ToolError::McpGetToolError(Box::new(error)))
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
#[path = "server_tests.rs"]
mod tests;
