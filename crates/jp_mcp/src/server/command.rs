//! Tools served by command plugins.
//!
//! A tool whose source is `command.<plugin>` is answered by a plugin binary,
//! run once per attempt the way a local tool's command is: its stdin carries
//! one `init` message and is then closed, and its stdout carries back one
//! `tool_outcome` and an `exit`.
//!
//! Which binaries may run is decided by the host before the turn starts, and
//! handed in as [`CommandPlugins`]: each admitted plugin's binary, the hash of
//! the contents that were admitted, and the plugin's options as the turn
//! resolved them.
//! Nothing here decides trust; a plugin absent from [`CommandPlugins`] does not
//! run.
//!
//! See: `docs/rfd/072-command-plugin-system.md`, "Tool Calls".

use std::{collections::HashMap, fs, io, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use indexmap::IndexMap;
use jp_config::types::json_value::JsonValue;
use jp_plugin::{
    PROTOCOL_VERSION,
    message::{
        HostToPlugin, InitMessage, LogMessage, OutputFormat, PathsInfo, PluginToHost, ToolAction,
        ToolCall, WorkspaceInfo,
    },
};
use jp_process::LineSink;
use jp_tool::{AccessPolicy, Action, Error as ToolError, InvocationContext};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};
use tracing::{debug, error, info, trace, warn};

use super::{Answers, StderrSink};

/// The `init` fields every plugin a turn runs is told, whichever tool it
/// answers.
#[derive(Debug, Clone, Default)]
pub struct PluginInit {
    /// The workspace's globally unique ID.
    pub workspace_id: String,

    /// The `.jp` storage directory.
    ///
    /// `None` when filesystem storage is not configured, which leaves a plugin
    /// with nowhere it is told to store anything; its calls then fail.
    pub storage: Option<Utf8PathBuf>,

    /// Well-known JP directories.
    pub paths: PathsInfo,

    /// The turn's resolved configuration, as the `init` message carries it.
    pub config: Value,

    /// The host's log verbosity, so plugin stderr matches `-v`.
    pub log_level: u8,
}

/// A plugin binary the host admitted for a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedPlugin {
    /// The binary.
    pub binary: Utf8PathBuf,

    /// The SHA-256 of the contents admission decided on, from [`sha256_file`].
    pub sha256: String,

    /// `plugins.command.<plugin>.options` as the turn resolved them.
    pub options: Map<String, Value>,
}

/// The command plugins a turn may run, and what each call's `init` carries.
#[derive(Debug, Clone, Default)]
pub struct CommandPlugins {
    init: PluginInit,
    admitted: HashMap<String, AdmittedPlugin>,
}

impl CommandPlugins {
    /// No plugin admitted yet, with `init` shared by every call.
    #[must_use]
    pub fn new(init: PluginInit) -> Self {
        Self {
            init,
            admitted: HashMap::new(),
        }
    }

    /// Let the plugin `name` run this turn.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, plugin: AdmittedPlugin) -> Self {
        self.admitted.insert(name.into(), plugin);
        self
    }

    /// The admitted binary for `plugin`, provided it still holds the contents
    /// that were admitted.
    ///
    /// # Errors
    ///
    /// The reason it may not run, as a sentence for
    /// [`ToolError::CommandPluginUnavailable`]: it was not admitted, or the
    /// binary changed since.
    pub fn verify(&self, plugin: &str) -> Result<&AdmittedPlugin, String> {
        let admitted = self
            .admitted
            .get(plugin)
            .ok_or("it was not admitted for this turn")?;

        let sha256 = sha256_file(&admitted.binary)
            .map_err(|error| format!("cannot read {}: {error}", admitted.binary))?;
        if sha256 != admitted.sha256 {
            return Err(format!(
                "{} changed since it was admitted at the start of this turn",
                admitted.binary
            ));
        }

        Ok(admitted)
    }

    /// The `init` that starts one attempt of `call` on `plugin`.
    pub(super) fn init_message(
        &self,
        plugin: &AdmittedPlugin,
        call: &CommandToolCall<'_>,
    ) -> Result<InitMessage, ToolError> {
        let storage =
            self.init
                .storage
                .clone()
                .ok_or_else(|| ToolError::CommandPluginUnavailable {
                    plugin: call.plugin.to_owned(),
                    reason: "the workspace has no storage configured".to_owned(),
                })?;

        Ok(InitMessage {
            version: PROTOCOL_VERSION,
            workspace: WorkspaceInfo {
                root: call.root.to_owned(),
                storage,
                id: self.init.workspace_id.clone(),
            },
            paths: self.init.paths.clone(),
            config: self.init.config.clone(),
            options: plugin.options.clone(),
            args: vec![],
            log_level: self.init.log_level,
            // A tool's output goes to the model, which reads plain text.
            output_format: OutputFormat::Text,
            tool: Some(tool_call(call)?),
        })
    }
}

/// The SHA-256 of a file's contents, as lowercase hex.
///
/// # Errors
///
/// When the file cannot be read.
pub fn sha256_file(path: &Utf8Path) -> io::Result<String> {
    Ok(format!("{:x}", Sha256::digest(fs::read(path)?)))
}

/// One attempt of a tool call routed to a command plugin.
pub(super) struct CommandToolCall<'a> {
    /// Whether the plugin runs the tool or describes the call.
    pub action: &'a Action,

    /// The plugin serving the tool, as named under `plugins.command`.
    pub plugin: &'a str,

    /// The name the plugin knows the tool by.
    pub tool: &'a str,

    /// Arguments after schema coercion, defaults, and validation.
    pub arguments: &'a Map<String, Value>,

    /// Answers to questions earlier attempts of this call asked.
    pub answers: &'a Answers,

    /// The tool's own `conversation.tools.<name>.options`.
    pub options: &'a IndexMap<String, JsonValue>,

    /// The workspace root the call runs against.
    pub root: &'a Utf8Path,

    /// The tool's compiled access grants, which the plugin must enforce.
    ///
    /// `None` means the tool declares no policy: unrestricted but
    /// workspace-confined filesystem access, as for a local tool.
    pub access: Option<&'a AccessPolicy>,

    /// Workspace and conversation the call belongs to.
    pub invocation: &'a InvocationContext,
}

/// The call as the protocol carries it.
///
/// Fails rather than sending a call without its access policy: a plugin that
/// received none would read it as unrestricted.
fn tool_call(call: &CommandToolCall<'_>) -> Result<ToolCall, ToolError> {
    let access = call
        .access
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| ToolError::CommandPluginFailed {
            plugin: call.plugin.to_owned(),
            message: format!("failed to serialize the access policy: {error}"),
        })?;

    Ok(ToolCall {
        action: match call.action {
            Action::Run => ToolAction::Run,
            Action::FormatArguments => ToolAction::FormatArguments,
        },
        name: call.tool.to_owned(),
        arguments: call.arguments.clone(),
        answers: call
            .answers
            .iter()
            .map(|(id, answer)| (id.clone(), answer.clone()))
            .collect(),
        options: call
            .options
            .iter()
            .map(|(key, JsonValue(value))| (key.clone(), value.clone()))
            .collect(),
        access,
        conversation: call.invocation.conversation_id.clone(),
    })
}

/// The `init` line written to the plugin's stdin.
pub(super) fn init_line(init: InitMessage, plugin: &str) -> Result<String, ToolError> {
    let mut line = serde_json::to_string(&HostToPlugin::Init(Box::new(init))).map_err(|error| {
        ToolError::CommandPluginFailed {
            plugin: plugin.to_owned(),
            message: format!("failed to serialize the init message: {error}"),
        }
    })?;
    line.push('\n');
    Ok(line)
}

/// Read what a plugin printed on stdout, returning the outcome it sent.
///
/// The error is a sentence about the plugin, for
/// [`ToolError::CommandPluginFailed`].
pub(super) fn parse_plugin_output(stdout: &str) -> Result<Value, String> {
    let mut outcome = None;

    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let message: PluginToHost = serde_json::from_str(line).map_err(|error| {
            format!("sent a line that is not a protocol message: {error}: {line}")
        })?;
        trace!(?message, "Plugin message during a tool call.");

        match message {
            PluginToHost::Ready(ready) if ready.protocol > PROTOCOL_VERSION => {
                return Err(format!(
                    "it needs `jp` protocol {}, and this `jp` speaks {PROTOCOL_VERSION}",
                    ready.protocol
                ));
            }
            PluginToHost::Log(log) => emit_log(&log),
            PluginToHost::ToolOutcome(message) => outcome = Some(message.outcome),
            PluginToHost::Exit(exit) if exit.code != 0 => {
                return Err(exit
                    .reason
                    .unwrap_or_else(|| format!("exited with code {}", exit.code)));
            }
            PluginToHost::Exit(_) => break,

            // A tool's result is its outcome, and its input closed after
            // `init`: printed output and requests have nowhere to go.
            other => debug!(?other, "Ignoring a plugin message during a tool call."),
        }
    }

    outcome.ok_or_else(|| {
        format!(
            "exited without answering the tool call; it may predate `jp` protocol \
             {PROTOCOL_VERSION}"
        )
    })
}

/// Forward each line of a plugin's stderr to tracing, and to `sink` when the
/// turn shows it.
///
/// Blank lines carry nothing to show, so neither sees them.
pub(super) fn plugin_stderr(plugin: String, tool: String, sink: Option<StderrSink>) -> LineSink {
    Arc::new(move |line: &str| {
        if line.is_empty() {
            return;
        }
        trace!(target: "plugin::stderr", plugin = %plugin, tool = %tool, "{line}");
        if let Some(sink) = &sink {
            sink(line);
        }
    })
}

/// Forward a plugin's log message to tracing at the level it named.
fn emit_log(log: &LogMessage) {
    let message = &log.message;
    match log.level.as_str() {
        "trace" => trace!(target: "plugin", message = %message),
        "debug" => debug!(target: "plugin", message = %message),
        "info" => info!(target: "plugin", message = %message),
        "warn" => warn!(target: "plugin", message = %message),
        "error" => error!(target: "plugin", message = %message),
        level => warn!(target: "plugin", level, message = %message, "unknown log level"),
    }
}

#[cfg(test)]
#[path = "command_tests.rs"]
mod tests;
