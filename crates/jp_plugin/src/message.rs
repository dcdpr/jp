//! Protocol message types for the JP plugin system.
//!
//! Messages are exchanged as JSON-lines (one JSON object per line) over stdin
//! (host→plugin) and stdout (plugin→host).

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Well-known JP directory paths.
///
/// Provided in the `init` message so plugins can locate JP data directories
/// without depending on platform-specific path resolution logic.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PathsInfo {
    /// User-local data directory.
    ///
    /// Platform-specific base directory for JP's persistent data:
    ///
    /// - Linux: `$XDG_DATA_HOME/jp` (typically `~/.local/share/jp`)
    /// - macOS: `~/Library/Application Support/jp`
    /// - Windows: `{FOLDERID_LocalAppData}\jp\data`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_data: Option<Utf8PathBuf>,

    /// User-global config directory.
    ///
    /// Where JP looks for global configuration files.
    /// May differ from `user_data` on Linux (XDG config vs data) and Windows
    /// (Roaming vs Local `AppData`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_config: Option<Utf8PathBuf>,

    /// User-local workspace storage directory.
    ///
    /// Per-workspace user data (e.g. local config overrides, session state).
    /// `None` if local storage is not configured for this workspace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_workspace: Option<Utf8PathBuf>,
}

/// Messages sent from the host (`jp`) to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostToPlugin {
    /// Sent immediately after spawning the plugin.
    Init(InitMessage),

    /// Response to `list_conversations`.
    Conversations(ConversationsResponse),

    /// Response to `read_events`.
    Events(EventsResponse),

    /// Response to `read_config`.
    Config(ConfigResponse),

    /// Response to `compose`.
    Composed(ComposeResponse),

    /// An error response to any plugin request.
    Error(ErrorResponse),

    /// Request plugin metadata (name, version, description, help text).
    ///
    /// Sent instead of `Init` when the host only needs the plugin's
    /// self-description (e.g. for `jp -h` or `jp <plugin> -h`).
    /// The plugin should respond with `PluginToHost::Describe` and exit.
    Describe,

    /// Graceful shutdown request (e.g. SIGINT/SIGTERM received).
    Shutdown,
}

/// Messages sent from the plugin to the host (`jp`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginToHost {
    /// Acknowledge successful initialization, and state what the plugin needs.
    Ready(ReadyMessage),

    /// Request a list of conversations.
    ListConversations(OptionalId),

    /// Request events for a conversation.
    ReadEvents(ReadEventsRequest),

    /// Request the resolved config (or a subtree).
    ReadConfig(ReadConfigRequest),

    /// Ask the host to collect text from the user.
    Compose(ComposeRequest),

    /// Print user-facing output through JP's printer.
    Print(PrintMessage),

    /// Emit a structured log message.
    Log(LogMessage),

    /// Respond with plugin metadata.
    Describe(DescribeResponse),

    /// Signal that the plugin is done.
    Exit(ExitMessage),
}

// --- Host-to-Plugin messages ---

/// The `init` message sent to the plugin on startup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InitMessage {
    /// Protocol version.
    /// Plugins should check compatibility.
    pub version: u32,

    /// Workspace information.
    pub workspace: WorkspaceInfo,

    /// Well-known JP directory paths.
    ///
    /// Allows plugins to locate user data, config, and workspace directories
    /// without platform-specific logic.
    #[serde(default)]
    pub paths: PathsInfo,

    /// The fully resolved `AppConfig` as JSON.
    pub config: Value,

    /// Plugin-specific options from the host configuration.
    ///
    /// Contains the `options` map from the plugin's `CommandPluginConfig`, if
    /// any.
    /// Empty when no options are configured.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub options: Map<String, Value>,

    /// Remaining CLI arguments after the subcommand name.
    #[serde(default)]
    pub args: Vec<String>,

    /// The host's log verbosity level (0 = error, 1 = warn, ..., 4 = trace).
    ///
    /// Plugins should use this to configure their own tracing subscriber so
    /// that stderr output matches the host's `-v` flags.
    #[serde(default)]
    pub log_level: u8,
}

/// Workspace metadata included in the `init` message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceInfo {
    /// Absolute path to the workspace root.
    pub root: Utf8PathBuf,

    /// Absolute path to the `.jp` storage directory.
    pub storage: Utf8PathBuf,

    /// The workspace's globally unique ID.
    pub id: String,
}

/// Summary of a conversation, returned in `conversations` responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationSummary {
    /// The conversation ID (decisecond timestamp string).
    pub id: String,

    /// The conversation title, if any.
    pub title: Option<String>,

    /// When the conversation was last activated.
    pub last_activated_at: DateTime<Utc>,

    /// When the conversation was pinned, absent if it is not pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned_at: Option<DateTime<Utc>>,

    /// Number of events in the conversation.
    pub events_count: usize,
}

/// Response to `list_conversations`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationsResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The list of conversations.
    pub data: Vec<ConversationSummary>,
}

/// Response to `read_events`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventsResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation ID.
    pub conversation: String,

    /// Serialized conversation events.
    pub data: Vec<Value>,
}

/// Response to `read_config`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The config path that was requested, if narrowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// The config data.
    pub data: Value,
}

/// An error response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The type of the failed request.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,

    /// Human-readable error message.
    pub message: String,
}

// --- Plugin-to-Host messages ---

/// The plugin's answer to `init`.
///
/// Carrying the required protocol version here rather than leaving each plugin
/// to check for itself means the host can refuse a plugin it is too old to
/// serve, and a plugin cannot forget to ask: the field has no Rust default, so
/// it has to be named at every construction site.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadyMessage {
    /// The lowest protocol version this plugin can work with.
    ///
    /// Absent on the wire, it reads as 1.
    /// That is an assumption about a plugin built before the field existed, not
    /// something the plugin said: such a plugin has no way to report what it
    /// needs, and may well need more than 1 — one built against protocol 2
    /// uses `compose` and still sends a bare `ready`.
    /// So this is the one mismatch the handshake cannot catch, and it surfaces
    /// later as a message the host cannot parse.
    #[serde(default = "legacy_protocol")]
    pub protocol: u32,
}

const fn legacy_protocol() -> u32 {
    1
}

/// A message with only an optional correlation ID.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OptionalId {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// Request to read events for a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadEventsRequest {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation ID.
    pub conversation: String,
}

/// Request to read config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReadConfigRequest {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Optional dot-separated path to narrow the config response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Ask the host to collect text from the user.
///
/// Composition happens on the host rather than in the plugin because the host
/// owns both ends of it: a plugin's stdin carries this protocol, so it has no
/// terminal to read keys from, and only the host knows which editor the
/// `Ctrl+X` escape should open.
///
/// The host answers with [`HostToPlugin::Composed`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComposeRequest {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Rendered before the input, naming what is being asked for.
    pub message: String,

    /// What kind of input to collect.
    pub mode: ComposeMode,

    /// Help text rendered alongside the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

/// What a [`ComposeRequest`] asks for.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ComposeMode {
    /// A single line, pre-filled with `default`.
    Line {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },

    /// A multi-line buffer seeded with `initial_text`, offering the `Ctrl+X`
    /// escape to the user's configured editor.
    Buffer {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        initial_text: Option<String>,
    },

    /// One of a fixed set of choices.
    ///
    /// The response carries the chosen option's `value`, not its label.
    Select {
        options: Vec<ComposeOption>,

        /// The `value` to start the selection on.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },

    /// Any number of a fixed set of choices.
    ///
    /// The response carries the chosen `value`s in [`ComposeResponse::values`].
    MultiSelect { options: Vec<ComposeOption> },
}

/// One choice in a [`ComposeMode::Select`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComposeOption {
    /// What the plugin gets back when this is chosen.
    pub value: String,

    /// What the user reads.
    pub label: String,
}

/// Response to `compose`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComposeResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// What the user wrote, or the single value they chose.
    ///
    /// `None` when they cancelled, when there was no terminal to ask on, or
    /// when the request was a multi-select.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    /// The values chosen from a [`ComposeMode::MultiSelect`].
    ///
    /// Empty for every other mode, and for a cancelled or unanswerable one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
}

/// Print user-facing output through JP's printer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PrintMessage {
    /// The text to print.
    pub text: String,

    /// Output channel (default: "content").
    #[serde(default = "default_channel")]
    pub channel: String,

    /// Text format (default: "plain").
    #[serde(default = "default_format")]
    pub format: String,

    /// Language hint for `code` format.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// A structured log message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogMessage {
    /// Log level: trace, debug, info, warn, error.
    pub level: String,

    /// The log message.
    pub message: String,

    /// Optional structured fields.
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub fields: serde_json::Map<String, Value>,
}

/// Plugin metadata returned in response to a `Describe` request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DescribeResponse {
    /// The plugin's display name (e.g. "serve").
    pub name: String,

    /// Plugin version string.
    pub version: String,

    /// One-line description for command listings.
    pub description: String,

    /// The command path this plugin provides.
    ///
    /// Each element is a subcommand segment.
    /// For example, `["serve", "web"]` means the plugin handles `jp serve web`.
    /// When absent, the host derives the path from the binary name by stripping
    /// the `jp-` prefix and splitting on `-`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,

    /// Plugin author.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,

    /// Full help text shown for `jp <plugin> -h`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,

    /// Repository URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
}

/// The plugin is done and wants JP to exit with this code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExitMessage {
    /// The exit code.
    pub code: u8,

    /// Human-readable reason for a non-zero exit.
    ///
    /// When present and the code is non-zero, the host prints this to the user.
    /// Omit for successful exits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn default_channel() -> String {
    "content".to_owned()
}

fn default_format() -> String {
    "plain".to_owned()
}

#[cfg(test)]
#[path = "message_tests.rs"]
mod tests;
