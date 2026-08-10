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

    /// A request that only reports whether it worked, worked.
    ///
    /// The answer to `archive_conversation` and `set_title`.
    /// A failure comes back as [`HostToPlugin::Error`] instead, naming which
    /// request it was.
    Done(DoneResponse),

    /// Response to `read_draft` and `write_draft`.
    Draft(DraftResponse),

    /// Response to `list_configs`.
    Configs(ConfigsResponse),

    /// A delegated turn finished.
    QueryComplete(QueryCompleteResponse),

    /// A conversation asked for by `query` with `new` exists.
    ///
    /// Sent as soon as it has been created and locked, before its first turn
    /// runs.
    /// A caller that only needs somewhere to send the user cannot wait for the
    /// turn: it can take minutes, and the conversation is usable immediately.
    ///
    /// The first of two replies to such a request; `query_complete` follows
    /// when the turn ends.
    Created(CreatedResponse),

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

    /// Move a conversation to the archive.
    ArchiveConversation(ConversationRequest),

    /// Rename a conversation, or clear its name.
    SetTitle(SetTitleRequest),

    /// Read a conversation's query draft.
    ReadDraft(ConversationRequest),

    /// Replace a conversation's query draft.
    WriteDraft(WriteDraftRequest),

    /// List the configurations a query can name.
    ListConfigs(OptionalId),

    /// Ask the host to run a turn on a conversation.
    Query(QueryRequest),

    /// Ask the host to interrupt the turn running on a conversation.
    Interrupt(InterruptRequest),

    /// Print user-facing output through JP's printer.
    Print(PrintMessage),

    /// Emit a structured log message.
    Log(LogMessage),

    /// Respond with plugin metadata.
    Describe(DescribeResponse),

    /// Signal that the plugin is done.
    Exit(ExitMessage),
}

impl PluginToHost {
    /// The correlation ID this message carries, if any.
    ///
    /// `None` covers two cases that need no distinguishing here: a request that
    /// omitted its optional id, and a message that is not a request at all.
    /// Neither has an answer to correlate.
    ///
    /// Exists so a host can answer a request it could not otherwise inspect,
    /// notably when handling it failed and the failure has to be reported
    /// against the right request rather than sent into the void for the plugin
    /// to time out on.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::ListConversations(m) | Self::ListConfigs(m) => m.id.as_deref(),
            Self::ReadEvents(m) => m.id.as_deref(),
            Self::ReadConfig(m) => m.id.as_deref(),
            Self::Compose(m) => m.id.as_deref(),
            Self::Query(m) => m.id.as_deref(),
            Self::ArchiveConversation(m) | Self::ReadDraft(m) => m.id.as_deref(),
            Self::SetTitle(m) => m.id.as_deref(),
            Self::WriteDraft(m) => m.id.as_deref(),

            // Not requests: nothing is waiting on an answer to any of these.
            Self::Ready(_)
            | Self::Interrupt(_)
            | Self::Print(_)
            | Self::Log(_)
            | Self::Describe(_)
            | Self::Exit(_) => None,
        }
    }
}

// --- Host-to-Plugin messages ---

/// How the host renders what it prints.
///
/// A plugin reads this to decide the shape of its own output, so `jp --format
/// json` reaches a plugin's listings the way it reaches the host's own commands
/// and a caller does not have to learn a separate flag per plugin.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// Plain text, with no ANSI colors and no unicode decoration.
    #[default]
    Text,

    /// Text with ANSI colors and unicode decoration.
    TextPretty,

    /// Compact JSON, one line per print.
    Json,

    /// Indented JSON.
    JsonPretty,
}

impl OutputFormat {
    /// Whether output should be machine-readable.
    #[must_use]
    pub const fn is_json(self) -> bool {
        matches!(self, Self::Json | Self::JsonPretty)
    }

    /// Whether JSON output should be indented.
    #[must_use]
    pub const fn is_json_pretty(self) -> bool {
        matches!(self, Self::JsonPretty)
    }

    /// Whether text output can carry ANSI colors and unicode decoration.
    #[must_use]
    pub const fn is_pretty(self) -> bool {
        matches!(self, Self::TextPretty)
    }
}

/// Who holds a conversation.
///
/// A conversation is locked for the length of a turn, so this says whether one
/// is running, and whether it is the reader's to interrupt.
/// A turn in another process can be waited for but not signalled from here.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LockState {
    /// Nobody.
    /// No turn is running.
    #[default]
    Free,

    /// A turn in another process.
    Elsewhere,

    /// A turn in the host answering this request.
    Here,
}

impl LockState {
    /// Whether no turn is running.
    ///
    /// Takes a reference because `skip_serializing_if` calls it with one.
    #[must_use]
    pub const fn is_free(&self) -> bool {
        matches!(self, Self::Free)
    }

    /// Whether a turn is running, wherever it is.
    #[must_use]
    pub const fn is_held(&self) -> bool {
        !self.is_free()
    }

    /// Whether the running turn can be interrupted through this connection.
    #[must_use]
    pub const fn is_here(&self) -> bool {
        matches!(self, Self::Here)
    }
}

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

    /// The shape the host's own output takes, resolved from `--format`.
    ///
    /// A plugin that prints listings or records should match it, so one flag
    /// governs the whole invocation.
    ///
    /// Reads as [`OutputFormat::Text`] when the host is old enough not to send
    /// it, which is the shape plugins printed before they could ask.
    /// That fallback is why this needs no protocol version of its own: there is
    /// nothing a plugin has to refuse to run without.
    #[serde(default)]
    pub output_format: OutputFormat,
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

    /// Who holds this conversation, if anyone.
    ///
    /// Read from the conversation lock, which is the only authoritative answer:
    /// a transcript ending in a request looks identical whether a turn is
    /// running, was interrupted, or failed outright.
    #[serde(default, skip_serializing_if = "LockState::is_free")]
    pub lock: LockState,

    /// The conversation's title, if it has one.
    ///
    /// Saves a plugin from asking for the whole conversation list to label one
    /// conversation, which reads every conversation's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

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

/// A request that only reports whether it worked, worked.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DoneResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// A conversation's query draft.
///
/// The answer to both `read_draft` and `write_draft`, so a write reports back
/// what the draft now holds without a second read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DraftResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation the draft belongs to.
    pub conversation: String,

    /// The draft text, empty when there is no draft.
    pub content: String,

    /// A fingerprint of `content`, absent when there is no draft.
    ///
    /// Passed back in the next `write_draft` to say which version was edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,

    /// Whether a `write_draft` was refused because the draft had moved on.
    ///
    /// When true, `content` and `revision` describe what is on disk, not what
    /// was submitted, and the write did not happen.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub conflict: bool,
}

/// Replace a conversation's query draft.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WriteDraftRequest {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation the draft belongs to.
    pub conversation: String,

    /// The new draft text.
    /// Empty removes the draft.
    pub content: String,

    /// The `revision` the edit was based on, from an earlier draft response.
    ///
    /// Absent means "there was no draft when I started".
    /// A mismatch against what is on disk is refused rather than overwritten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// Ask the host to run a turn on a conversation.
///
/// The host owns the agent loop: it locks the conversation, appends the
/// request, calls the provider, runs whatever tools the assistant asks for, and
/// persists the result.
/// A plugin doing this itself would need the user's credentials, the tool
/// registry, and the MCP servers, and would end up a second implementation of
/// the turn loop.
///
/// The host answers with [`HostToPlugin::QueryComplete`] once the turn has
/// finished, or [`HostToPlugin::Error`] if it could not be started.
/// A turn runs for as long as the assistant needs, so a plugin awaiting the
/// reply should allow minutes, not seconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryRequest {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation to add the turn to.
    ///
    /// Ignored when `new` is set, which makes the host create one.
    #[serde(default)]
    pub conversation: String,

    /// What the user said.
    pub content: String,

    /// Start a new conversation rather than adding to an existing one.
    ///
    /// The host replies twice: [`HostToPlugin::Created`] as soon as the
    /// conversation exists, carrying the id it assigned, then
    /// [`HostToPlugin::QueryComplete`] when the turn ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub new: bool,

    /// A title for a conversation created by this request.
    ///
    /// Left unset, a new conversation is untitled until the title generator
    /// names it from the first turn.
    /// Ignored without `new`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Configurations to layer over what this turn would otherwise run under,
    /// as `--cfg` takes them.
    ///
    /// Not scoped to the turn.
    /// The host records the difference as a config event, so the choice holds
    /// for the turns after it and the stream carries the reason, which is what
    /// `jp q --cfg` does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cfg: Vec<String>,
}

/// Ask the host to interrupt the turn running on a conversation.
///
/// Reaches the turn the same way a Ctrl-C from a terminal would, so it
/// escalates on repeat exactly as the terminal does: the first asks the turn to
/// wrap up, and pressing on abandons it.
///
/// Fire-and-forget: the host sends no acknowledgement, because what the
/// interrupt did shows up in the conversation itself.
/// The outcome of the turn still arrives as the reply to the original `query`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InterruptRequest {
    /// The conversation whose turn should stop.
    ///
    /// Required, and not a convenience: a host can be running several turns at
    /// once, so there is no "the" turn to infer.
    pub conversation: String,
}

/// Response to `query`, sent once the turn has finished.
///
/// Carries no transcript: the events are persisted, and the plugin reads them
/// back with [`PluginToHost::ReadEvents`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QueryCompleteResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation the turn ran on.
    pub conversation: String,
}

/// The conversation a `query` with `new` created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreatedResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The id the host assigned, which the caller has no other way to learn.
    pub conversation: String,
}

/// One configuration a query can name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigEntry {
    /// What to pass to select it, e.g. `skill/rfd`.
    pub segment: String,

    /// The directories above it, e.g. `skill`.
    /// Empty at the top level.
    pub namespace: String,

    /// The last part of the segment, e.g. `rfd`.
    pub name: String,
}

/// Response to `list_configs`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigsResponse {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Every selectable configuration, sorted by segment.
    pub data: Vec<ConfigEntry>,
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
    /// Defaults to 1 on the wire, so a plugin built before this field existed
    /// still parses, and is taken at its word.
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

/// A request naming one conversation and nothing else.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationRequest {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation to act on.
    pub conversation: String,
}

/// Rename a conversation, or clear its name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SetTitleRequest {
    /// Optional request correlation ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The conversation to rename.
    pub conversation: String,

    /// The new title.
    ///
    /// Absent, or blank, clears it: the conversation is then eligible for a
    /// generated title again rather than being named the empty string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
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
