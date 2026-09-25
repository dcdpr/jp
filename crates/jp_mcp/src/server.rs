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
mod http_client;
pub mod json_schema;
pub mod result;
pub mod service;
mod upstream;
use std::{fmt, sync::Arc};

pub use builtin::BuiltinTool;
use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::{
    conversation::tool::{
        CommandConfig, ToolConfigWithDefaults, ToolSource, style::ParametersStyle,
    },
    types::command::shell_command_line,
};
use jp_process::{Ended, LineSink, ProcessRunner, ProcessSpec, Watch};
use jp_tool::{
    AccessPolicy, Action, Error as ToolError, InvocationContext, Outcome, ParameterDocs, Question,
    ToolDefinition, ToolDocs, ToolResult,
    content::{ErrorDetails, ToolStatus},
    definition::{apply_parameter_defaults, split_description, validate_tool_arguments},
    schema::{Node, merge_description},
};
use minijinja::{Environment, ErrorKind as MinijinjaErrorKind, value::ValueKind};
use result::from_mcp;
use serde_json::{Error as JsonError, Map, Value, json};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, trace, warn};
use upstream::{UpstreamResult, decode_result, replace_envelope};

use crate::{
    Client,
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
#[derive(Debug)]
pub enum ExecutionOutcome {
    /// Tool executed and produced a result.
    Completed {
        /// The tool call ID (for correlation with the request).
        id: String,

        /// The execution result.
        ///
        /// If an error occurred, it means the tool ran, but reported an error.
        result: ToolResult,
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
        matches!(self, Self::Completed { result, .. } if !result.is_error())
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
    FatalError {
        /// Original error envelope, retained for the current conversation
        /// format.
        raw: String,
        /// Source chain reported by the tool.
        trace: Vec<String>,
    },

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
        detail: JsonError,
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

    /// Convert command output to ordered content with a typed status.
    ///
    /// # Panics
    ///
    /// Panics on `NeedsInput`, which must be handled before final delivery.
    pub fn into_tool_result(self, name: &str) -> ToolResult {
        match self {
            Self::Success(content) => ToolResult::text(content),
            Self::TransientError { message, trace } => {
                let mut result =
                    ToolResult::error(json!({"message": message, "trace": trace}).to_string());
                result.status = ToolStatus::Error(ErrorDetails {
                    transient: true,
                    trace,
                });
                result
            }
            Self::FatalError { raw, trace } => {
                let mut result = ToolResult::error(raw);
                result.status = ToolStatus::Error(ErrorDetails {
                    transient: false,
                    trace,
                });
                result
            }
            Self::Cancelled => ToolResult::text("Tool execution cancelled by user."),
            Self::RawOutput {
                stdout,
                stderr,
                success,
            } => {
                if success {
                    ToolResult::text(stdout)
                } else {
                    ToolResult::error(
                        json!({
                            "message": format!("Tool '{name}' execution failed."),
                            "stderr": stderr,
                            "stdout": stdout,
                        })
                        .to_string(),
                    )
                }
            }
            Self::InvalidInquiry { question_id } => {
                error!(
                    tool = name,
                    question_id = %question_id,
                    "tool produced an invalid inquiry: question id must be non-empty and must not \
                     contain '.'"
                );
                ToolResult::error(
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
                ToolResult::error(format!(
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
/// Called from the process runner's read loop, so it must not block: the loop
/// has to keep draining or the child fills its pipe and the tool call never
/// completes.
/// A consumer that falls behind drops rather than stalls.
pub type StderrSink = LineSink;

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

/// Run a tool command with cancellation support.
///
/// This is the **single entry point** for running tool commands (both execution
/// and argument formatting).
/// It handles:
///
/// 1. Template rendering via [`minijinja`]
/// 2. Running the process through `runner`, on a blocking thread
/// 3. Cancellation via [`CancellationToken`]
/// 4. Parsing stdout as [`jp_tool::Outcome`]
/// 5. Forwarding the child's stderr to tracing (when `trace_as` is `Some`)
pub async fn run_tool_command(
    runner: &Arc<dyn ProcessRunner>,
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

    let mut spec = if shell {
        // `program` is shell syntax and used verbatim; `args` are shell-quoted
        // so multi-word arguments keep their boundaries.
        ProcessSpec::new(
            "sh",
            ["-c".to_owned(), shell_command_line(&program, &args)],
            root,
        )
    } else {
        ProcessSpec::new(program, args, root)
    };
    // A Ctrl-C at the terminal must not kill the tool: JP stops it through the
    // cancellation token, once the user has chosen what the interrupt means.
    spec.own_process_group = true;

    // The process runs on a blocking thread, which dropping this future would
    // not stop, so the token is cancelled on drop as well as by the caller.
    let cancellation = cancellation_token.child_token();
    let _stop_on_drop = cancellation.clone().drop_guard();
    let watch = Watch {
        stderr_lines: trace_as.map(stderr_lines),
        cancellation: Some(cancellation),
        ..Watch::default()
    };
    let run = {
        let runner = Arc::clone(runner);
        let spec = spec.clone();
        tokio::task::spawn_blocking(move || runner.execute(&spec, &watch))
    };

    let finished = match run.await {
        Ok(Ok(finished)) => finished,
        Ok(Err(error)) => {
            return Err(ToolError::SpawnError {
                command: spec.to_string(),
                error,
            });
        }
        Err(error) => {
            return Ok(CommandResult::RawOutput {
                stdout: String::new(),
                stderr: error.to_string(),
                success: false,
            });
        }
    };

    if finished.ended == Ended::Cancelled {
        return Ok(CommandResult::Cancelled);
    }

    let output = finished.output;
    Ok(parse_command_output(
        output.stdout.as_bytes(),
        output.stderr.as_bytes(),
        output.success(),
    ))
}

/// Forward each line of a tool's stderr to tracing, and to the display
/// `trace_as` names, if any.
///
/// Blank lines carry nothing to show, so neither sees them.
fn stderr_lines(trace_as: ToolTrace<'_>) -> LineSink {
    let ToolTrace { id, name, stderr } = trace_as;
    let (id, name) = (id.to_owned(), name.to_owned());
    Arc::new(move |line: &str| {
        if line.is_empty() {
            return;
        }
        trace!(target: "tool::stderr", tool_id = %id, tool_name = %name, "{line}");
        if let Some(sink) = &stderr {
            sink(line);
        }
    })
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
                CommandResult::FatalError {
                    raw: stdout_str.into_owned(),
                    trace,
                }
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
            if !Outcome::claims_needs_input(&stdout_str) {
                return CommandResult::RawOutput {
                    stdout: stdout_str.into_owned(),
                    stderr: String::from_utf8_lossy(stderr).into_owned(),
                    success,
                };
            }

            match Outcome::claimed_question_id(&stdout_str) {
                // The id itself is the problem: empty, or containing the `.`
                // reserved as the inquiry-id separator (`QuestionId` rejects
                // both).
                Some(id) if id.is_empty() || id.contains('.') => {
                    CommandResult::InvalidInquiry { question_id: id }
                }
                // Some other field failed to parse (wrong shape, missing
                // field, protocol skew).
                _ => CommandResult::MalformedInquiry { detail: error },
            }
        }
    }
}

/// Everything an execution attempt needs, fixed for the life of one invocation.
///
/// A tool that asks for input ends its attempt and is run again once the answer
/// arrives, so [`execute`] takes the accumulated answers separately: they are
/// the only thing that differs between one attempt and the next.
pub struct Execution<'a> {
    /// The tool's advertised name and argument schema.
    pub definition: &'a ToolDefinition,

    /// Correlation id echoed back on the outcome.
    pub id: String,

    /// Arguments as the caller supplied them.
    /// Each attempt coerces its own copy to the schema.
    pub arguments: Value,

    /// What the tool is run for.
    ///
    /// [`Action::FormatArguments`] runs the tool's argument formatter, a local
    /// command configured in `style.parameters`, whatever the tool's own source
    /// is.
    pub action: Action,

    /// Where the tool comes from, and how it is configured to run.
    pub config: &'a ToolConfigWithDefaults,

    /// Working directory for a local command, and the root its access policy
    /// resolves paths against.
    pub root: &'a Utf8Path,

    /// Compiled access grants, or `None` for a tool that declares no policy.
    pub access: Option<&'a AccessPolicy>,

    /// Workspace and conversation the call belongs to.
    pub invocation: &'a InvocationContext,

    /// Rust implementations, reached by a `builtin` source.
    pub builtins: &'a builtin::BuiltinExecutors,

    /// Runs a local command: a `local` tool, or any tool's argument formatter.
    pub runner: &'a Arc<dyn ProcessRunner>,

    /// Upstream connections, reached by an `mcp` source.
    pub upstream: &'a Client,

    /// Stops the attempt in progress.
    pub cancellation: CancellationToken,

    /// Receives the tool's stderr lines as they arrive, for a caller showing
    /// progress.
    /// `None` when nothing is watching; the lines still reach tracing either
    /// way.
    pub stderr: Option<StderrSink>,
}

impl Execution<'_> {
    /// The name the tool's own implementation answers to, which is the
    /// configured `source` name when it differs from the advertised one.
    fn invoked_name<'n>(&'n self, source_name: Option<&'n str>) -> &'n str {
        source_name.unwrap_or(&self.definition.name)
    }

    /// Build the trusted template and metadata context for one attempt.
    fn context(&self, name: &str, arguments: &Value, answers: &Answers) -> Value {
        tool_context(
            name,
            arguments,
            answers,
            self.config,
            self.root,
            &self.action,
            self.access,
            self.invocation,
        )
    }
}

/// Answers a tool's earlier questions received, keyed by question id.
pub type Answers = IndexMap<String, Value>;

/// Run one execution attempt, without any interactive prompt.
///
/// Every interactive decision — admission, argument editing, who answers a
/// question, result review — belongs to the caller.
/// This resolves the tool's source, runs it once, and reports what came back.
///
/// An [`ExecutionOutcome::NeedsInput`] outcome ends the attempt.
/// Call again with the answer added to `answers` to run the tool a second time;
/// it is not suspended and resumed.
///
/// # Errors
///
/// Returns [`ToolError`] when the tool could not be run at all: a missing
/// command, a spawn failure, an unreachable MCP server, a malformed result
/// envelope.
/// A tool that ran and reported its own failure is an
/// [`ExecutionOutcome::Completed`] carrying an error [`ToolResult`], not an
/// `Err`.
pub async fn execute(
    execution: &Execution<'_>,
    answers: &Answers,
) -> Result<ExecutionOutcome, ToolError> {
    let mut arguments = execution.arguments.clone();
    if let Some(object) = arguments.as_object_mut() {
        execution.definition.coerce_arguments(object);
    }
    info!(
        tool = %execution.definition.name,
        action = ?execution.action,
        arguments = ?arguments,
        "Executing tool."
    );

    if execution.action.is_format_arguments() {
        let (ToolSource::Local { tool }
        | ToolSource::Builtin { tool }
        | ToolSource::Mcp { tool, .. }) = execution.config.source();
        let ParametersStyle::Custom(command) = &execution.config.style().parameters else {
            return Err(ToolError::MissingCommand);
        };
        let command = command.clone().command();
        return execute_local(execution, arguments, answers, tool.as_deref(), command).await;
    }

    match execution.config.source() {
        ToolSource::Local { tool } => {
            let Some(command) = execution.config.command() else {
                return Err(ToolError::MissingCommand);
            };
            execute_local(execution, arguments, answers, tool.as_deref(), command).await
        }
        ToolSource::Mcp { server, tool } => {
            execute_mcp(execution, arguments, answers, server, tool.as_deref()).await
        }
        ToolSource::Builtin { tool } => {
            execute_builtin(execution, &arguments, answers, tool.as_deref()).await
        }
    }
}

/// Run one attempt of a local command, and convert its result to an
/// `ExecutionOutcome`.
///
/// `command` is a local tool's own command, or any tool's argument formatter.
/// Arguments are defaulted and validated first either way.
async fn execute_local(
    execution: &Execution<'_>,
    mut arguments: Value,
    answers: &Answers,
    tool: Option<&str>,
    command: CommandConfig,
) -> Result<ExecutionOutcome, ToolError> {
    let name = execution.invoked_name(tool);
    let id = execution.id.clone();

    // Apply configured defaults for missing parameters, then validate.
    if let Some(args) = arguments.as_object_mut() {
        apply_parameter_defaults(args, &execution.definition.parameters);

        if let Err(error) = validate_tool_arguments(args, &execution.definition.parameters) {
            return Ok(ExecutionOutcome::Completed {
                id,
                result: ToolResult::error(format!(
                    "Invalid arguments: {error}\n\nYou can call `describe_tools(tools: \
                     [\"{name}\"])` to learn more about how to use the tool correctly."
                )),
            });
        }
    }

    let ctx = execution.context(name, &arguments, answers);

    let trace_as = ToolTrace {
        id: &id,
        name,
        stderr: execution.stderr.clone(),
    };

    let outcome = run_tool_command(
        execution.runner,
        command,
        ctx,
        execution.root,
        execution.cancellation.clone(),
        Some(trace_as),
    )
    .await?;

    match outcome {
        CommandResult::Success(content) => Ok(ExecutionOutcome::Completed {
            id,
            result: ToolResult::text(content),
        }),
        CommandResult::NeedsInput(question) => Ok(ExecutionOutcome::NeedsInput { id, question }),
        CommandResult::Cancelled => Ok(ExecutionOutcome::Cancelled { id }),
        other => Ok(ExecutionOutcome::Completed {
            id,
            result: other.into_tool_result(name),
        }),
    }
}

/// Execute an MCP tool and return the outcome.
///
/// Runs one upstream MCP call.
/// It calls the MCP server and converts the result to an `ExecutionOutcome`.
async fn execute_mcp(
    execution: &Execution<'_>,
    arguments: Value,
    answers: &Answers,
    server: &str,
    tool: Option<&str>,
) -> Result<ExecutionOutcome, ToolError> {
    let name = execution.invoked_name(tool);
    let id = execution.id.clone();

    let context = execution.context(name, &arguments, answers);
    let meta = Map::from_iter([
        ("computer.jp/tool".into(), context["tool"].clone()),
        ("computer.jp/context".into(), context["context"].clone()),
    ]);
    let call_future = execution
        .upstream
        .call_tool(name, server, &arguments, Some(meta));

    let response = tokio::select! {
        biased;
        () = execution.cancellation.cancelled() => {
            info!(tool = %execution.definition.name, "MCP tool call cancelled");
            return Ok(ExecutionOutcome::Cancelled { id });
        }
        result = call_future => result.map_err(|error| ToolError::McpRunToolError(Box::new(error)))?,
    };

    let result = match decode_result(response).map_err(ToolError::MalformedOutput)? {
        UpstreamResult::Native(response) => {
            from_mcp(response).map_err(ToolError::MalformedOutput)?
        }
        UpstreamResult::Outcome { outcome, response } => match outcome {
            Outcome::NeedsInput { question } => {
                return Ok(ExecutionOutcome::NeedsInput { id, question });
            }
            Outcome::Success { content } => from_mcp(replace_envelope(response, &content, false))
                .map_err(ToolError::MalformedOutput)?,
            Outcome::Error {
                message,
                trace,
                transient,
            } => {
                let text = if transient {
                    json!({"message":message, "trace":trace}).to_string()
                } else {
                    from_mcp(response.clone())
                        .map_err(ToolError::MalformedOutput)?
                        .to_text()
                };
                let mut result = from_mcp(replace_envelope(response, &text, true))
                    .map_err(ToolError::MalformedOutput)?;
                result.status = ToolStatus::Error(ErrorDetails { transient, trace });
                result
            }
        },
    };
    Ok(ExecutionOutcome::Completed { id, result })
}

/// Execute a builtin tool and return the outcome.
///
/// `source_name` is the implementation named by `source = "builtin.<name>"`,
/// which the registry is keyed on.
/// When absent, the implementation shares the tool's own name.
async fn execute_builtin(
    execution: &Execution<'_>,
    arguments: &Value,
    answers: &Answers,
    source_name: Option<&str>,
) -> Result<ExecutionOutcome, ToolError> {
    let name = execution.invoked_name(source_name);
    let id = execution.id.clone();
    let executor = execution
        .builtins
        .get(name)
        .ok_or_else(|| ToolError::NotFound {
            name: name.to_owned(),
        })?;

    let outcome = executor.execute(arguments, answers).await;

    Ok(match outcome {
        Outcome::Success { content } => ExecutionOutcome::Completed {
            id,
            result: ToolResult::text(content),
        },
        outcome @ Outcome::Error { .. } => ExecutionOutcome::Completed {
            id,
            result: outcome.into(),
        },
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
#[path = "server_testing.rs"]
pub(crate) mod testing;

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
