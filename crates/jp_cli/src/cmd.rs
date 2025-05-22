mod attachment;
mod conversation;
mod init;
mod mcp;
mod persona;
mod query;

use std::{borrow::Cow, collections::HashMap, fmt, num::NonZeroI32};

use comfy_table::Row;
use serde_json::{Map, Value};

use crate::Ctx;

#[derive(Debug, clap::Subcommand)]
#[expect(clippy::large_enum_variant)]
pub enum Commands {
    /// Initialize a new workspace.
    Init(init::Args),

    /// Query the assistant.
    #[command(visible_alias = "q")]
    Query(query::Args),

    /// Manage attachments.
    #[command(visible_alias = "a", alias = "attachments")]
    Attachment(attachment::Args),

    // TODO: Remove once we have proper customizable "command aliases".
    #[command(name = "aa", hide = true)]
    AttachmentAdd(attachment::add::Args),

    /// Manage personas.
    #[command(visible_alias = "p", alias = "personas")]
    Persona(persona::Args),

    /// Manage MCP servers.
    #[command(visible_alias = "m")]
    Mcp(mcp::Args),

    /// Manage conversations.
    #[command(visible_alias = "c", alias = "conversations")]
    Conversation(conversation::Args),
}

impl Commands {
    pub async fn run(self, ctx: &mut Ctx) -> Output {
        match self {
            Commands::Query(args) => args.run(ctx).await,
            Commands::Attachment(args) => args.run(ctx).await,
            Commands::AttachmentAdd(args) => args.run(ctx).await,
            Commands::Persona(args) => args.run(ctx),
            Commands::Mcp(args) => args.run(ctx),
            Commands::Conversation(args) => args.run(ctx).await,
            Commands::Init(_) => unreachable!("handled before workspace initialization"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Commands::Query(_) => "query",
            Commands::Attachment(_) => "attachment",
            Commands::AttachmentAdd(_) => "attachment-add",
            Commands::Persona(_) => "persona",
            Commands::Mcp(_) => "mcp",
            Commands::Init(_) => "init",
            Commands::Conversation(_) => "conversation",
        }
    }
}

pub(crate) type Output = std::result::Result<Success, Error>;

/// The type of output that should be printed to the screen.
#[derive(Debug)]
pub enum Success {
    /// The command was successful.
    Ok,

    /// Single message to be printed to the screen.
    Message(String),

    /// List of details to be printed in a table.
    Table { header: Row, rows: Vec<Row> },

    /// Details of a single item to be printed.
    Details {
        title: Option<String>,
        rows: Vec<Row>,
    },

    /// JSON value to be printed.
    Json(Value),
}

impl From<()> for Success {
    fn from(_value: ()) -> Self {
        Self::Ok
    }
}

impl From<String> for Success {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for Success {
    fn from(value: &str) -> Self {
        value.to_string().into()
    }
}

impl From<Cow<'_, str>> for Success {
    fn from(value: Cow<'_, str>) -> Self {
        value.to_string().into()
    }
}

impl From<Value> for Success {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, thiserror::Error)]
pub struct Error {
    /// The error code.
    ///
    /// Used to exit the CLI with a specific exit code. This is usually `1`.
    pub code: NonZeroI32,

    /// The optional error message to be displayed to the user.
    pub message: Option<String>,

    /// Metadata to be displayed to the user.
    ///
    /// This is hidden from the user in TTY mode, unless the `--verbose` flag is
    /// set.
    pub metadata: Map<String, Value>,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message.as_deref().unwrap_or_default())
    }
}

impl From<i32> for Error {
    fn from(code: i32) -> Self {
        Self {
            code: code.try_into().unwrap_or(NonZeroI32::new(1).unwrap()),
            message: None,
            metadata: Map::new(),
        }
    }
}

impl From<Box<dyn std::error::Error>> for Error {
    fn from(error: Box<dyn std::error::Error>) -> Self {
        Self::from(error.to_string())
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for Error {
    fn from(error: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::from(error.to_string())
    }
}

impl From<String> for Error {
    fn from(error: String) -> Self {
        (1, error).into()
    }
}

impl From<&str> for Error {
    fn from(error: &str) -> Self {
        error.to_owned().into()
    }
}

impl From<(i32, String)> for Error {
    fn from((code, message): (i32, String)) -> Self {
        (code, message, Map::new()).into()
    }
}

impl From<(i32, &str)> for Error {
    fn from((code, message): (i32, &str)) -> Self {
        (code, message.to_owned()).into()
    }
}

impl From<(i32, String, Map<String, Value>)> for Error {
    fn from((code, message, metadata): (i32, String, Map<String, Value>)) -> Self {
        Self {
            code: code.try_into().unwrap_or(NonZeroI32::new(1).unwrap()),
            message: Some(message),
            metadata,
        }
    }
}
impl From<(i32, &str, Map<String, Value>)> for Error {
    fn from((code, message, metadata): (i32, &str, Map<String, Value>)) -> Self {
        (code, message.to_string(), metadata).into()
    }
}

impl From<Map<String, Value>> for Error {
    fn from(metadata: Map<String, Value>) -> Self {
        (1, metadata).into()
    }
}

impl From<HashMap<&str, Value>> for Error {
    fn from(metadata: HashMap<&str, Value>) -> Self {
        metadata
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect::<HashMap<_, _>>()
            .into()
    }
}

impl From<HashMap<&'static str, String>> for Error {
    fn from(metadata: HashMap<&'static str, String>) -> Self {
        metadata
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect::<HashMap<_, _>>()
            .into()
    }
}

impl From<HashMap<String, Value>> for Error {
    fn from(metadata: HashMap<String, Value>) -> Self {
        (1, Map::from_iter(metadata)).into()
    }
}

impl From<(i32, Map<String, Value>)> for Error {
    fn from((code, mut metadata): (i32, Map<String, Value>)) -> Self {
        let message = metadata
            .remove("message")
            .and_then(|v| v.as_str().map(ToString::to_string))
            .unwrap_or_else(|| "Application error".to_owned());

        (code, message, metadata).into()
    }
}

impl From<crate::error::Error> for Error {
    fn from(error: crate::error::Error) -> Self {
        use crate::error::Error::*;

        let metadata: HashMap<&str, String> = match error {
            Command(error) => return error,
            Config(error) => return error.into(),
            Workspace(error) => return error.into(),
            Conversation(error) => return error.into(),
            Mcp(error) => return error.into(),
            Llm(error) => return error.into(),
            Io(error) => return error.into(),
            Url(error) => return error.into(),
            Bat(error) => return error.into(),
            Template(error) => return error.into(),
            Json(error) => return error.into(),
            NotFound(target, id) => [
                ("message", "Not found".into()),
                ("target", target.into()),
                ("id", id),
            ]
            .into(),
            Attachment(error) => [
                ("message", "Attachment error".into()),
                ("error", error.to_string()),
            ]
            .into(),
            Editor(error) => [
                ("message", "Editor error".into()),
                ("error", error.to_string()),
            ]
            .into(),
            Task(error) => with_cause(error.as_ref(), "Task error"),
            Replay(error) => [("message", "Replay error".to_owned()), ("error", error)].into(),
            TemplateUndefinedVariable(var) => [
                ("message", "Undefined template variable".to_owned()),
                ("variable", var),
            ]
            .into(),
            Schema(error) => [
                ("message", "Invalid JSON schema".to_owned()),
                ("error", error),
            ]
            .into(),
            MissingEditor => [("message", "Missing editor".to_owned())].into(),
        };

        Self::from(metadata)
    }
}

fn with_cause(
    error: &dyn std::error::Error,
    message: impl Into<String>,
) -> HashMap<&'static str, String> {
    let mut causes = vec![("message", message.into()), ("error", format!("{error:#}"))];

    while let Some(cause) = error.source() {
        causes.push(("", format!("{cause:#}")));
    }

    causes.into_iter().collect()
}

macro_rules! impl_from_error {
    ($error:ty, $message:expr) => {
        impl From<$error> for Error {
            fn from(error: $error) -> Self {
                with_cause(&error, $message).into()
            }
        }
    };
}

impl_from_error!(std::io::Error, "IO error");
impl_from_error!(minijinja::Error, "Template error");
impl_from_error!(bat::error::Error, "Error while formatting code");
impl_from_error!(url::ParseError, "Error while parsing URL");
impl_from_error!(serde_json::Error, "Error while parsing JSON");
impl_from_error!(reqwest::Error, "Error while making HTTP request");
impl_from_error!(std::str::ParseBoolError, "Error parsing boolean value");

impl From<jp_llm::Error> for Error {
    fn from(error: jp_llm::Error) -> Self {
        use jp_llm::Error::*;

        let metadata: HashMap<&str, String> = match error {
            OpenRouter(error) => return error.into(),
            Conversation(error) => return error.into(),
            Config(error) => return error.into(),
            Json(error) => return error.into(),
            Request(error) => return error.into(),
            Url(error) => return error.into(),
            MissingEnv(variable) => [
                ("message", "Missing environment variable".into()),
                ("variable", variable),
            ]
            .into(),
            InvalidResponse(error) => [
                ("message", "Invalid response received".into()),
                ("error", error),
            ]
            .into(),
            MissingStructuredData => {
                [("message", "Missing structured data in response".into())].into()
            }
            OpenaiClient(error) => with_cause(&error, "OpenAI client error"),
            OpenaiEvent(error) => with_cause(&error, "OpenAI stream error"),
            OpenaiResponse(error) => [
                ("message", "OpenAI response error".into()),
                ("error", error.message),
                ("code", error.code.unwrap_or_default()),
                ("type", error.r#type),
                ("param", error.param.unwrap_or_default()),
            ]
            .into(),
            OpenaiStatusCode {
                status_code,
                response,
            } => [
                ("message", "OpenAI status code error".into()),
                ("status_code", status_code.as_u16().to_string()),
                ("response", response),
            ]
            .into(),
            Anthropic(anthropic_error) => [
                ("message", "Anthropic error".into()),
                ("error", anthropic_error.to_string()),
            ]
            .into(),
            AnthropicRequestBuilder(error) => [
                ("message", "Anthropic request builder error".into()),
                ("error", error.to_string()),
            ]
            .into(),
            Ollama(error) => [
                ("message", "Ollama error".into()),
                ("error", error.to_string()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_openrouter::Error> for Error {
    fn from(error: jp_openrouter::Error) -> Self {
        use jp_openrouter::Error::*;

        let metadata: HashMap<&str, Value> = match error {
            Request(error) => return error.into(),
            Json(error) => return error.into(),
            Io(error) => return error.into(),
            Stream(string) => [
                ("message", "Error while processing stream".into()),
                ("error", string.into()),
            ]
            .into(),
            Api { code, message } => [
                ("message", "LLM Provider API error".into()),
                ("code", code.into()),
                ("message", message.into()),
            ]
            .into(),
            Config(message) => [
                ("message", "Config error".into()),
                ("error", message.into()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_config::Error> for Error {
    fn from(error: jp_config::Error) -> Self {
        use jp_config::Error::*;

        let metadata: HashMap<&str, Value> = match error {
            ParseBool(error) => return error.into(),
            Conversation(error) => return error.into(),
            Io(error) => return error.into(),
            Confique(error) => [
                ("message", "Config error".into()),
                ("error", error.to_string().into()),
            ]
            .into(),
            UnknownConfigKey {
                key,
                available_keys,
            } => [
                ("message", "Unknown config key".into()),
                ("key", key.into()),
                ("available_keys", available_keys.into()),
            ]
            .into(),
            InvalidConfigValue { key, value, need } => [
                ("message", "Invalid config value".into()),
                ("key", key.into()),
                ("value", value.into()),
                ("need", need.into()),
            ]
            .into(),
            ModelSlug(slug) => [
                ("message", "Invalid model slug".into()),
                ("slug", slug.into()),
            ]
            .into(),
            InvalidFileExtension { path } => [
                ("message", "Invalid or missing file extension".into()),
                ("path", path.to_string_lossy().into()),
            ]
            .into(),
            Toml(error) => [
                ("message", "TOML error".into()),
                ("error", error.to_string().into()),
            ]
            .into(),
            Json5(error) => [
                ("message", "JSON error".into()),
                ("error", error.to_string().into()),
            ]
            .into(),
            Yaml(error) => [
                ("message", "YAML error".into()),
                ("error", error.to_string().into()),
            ]
            .into(),
            Json(error) => return error.into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_mcp::Error> for Error {
    fn from(error: jp_mcp::Error) -> Self {
        use jp_mcp::Error::*;

        let metadata: HashMap<&str, Value> = match error {
            Service(service_error) => [
                ("message", "MCP service error".into()),
                ("error", service_error.to_string().into()),
            ]
            .into(),
            Timeout(elapsed) => [
                ("message", "MCP request timeout".into()),
                ("error", elapsed.to_string().into()),
            ]
            .into(),
            UnknownTool(tool) => [("message", "Unknown tool".into()), ("tool", tool.into())].into(),
            Io(error) => return error.into(),
            UnknownServer(mcp_server_id) => [
                ("message", "Unknown MCP server".into()),
                ("id", mcp_server_id.to_string().into()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_workspace::Error> for Error {
    fn from(error: jp_workspace::Error) -> Self {
        use jp_workspace::Error::*;

        let metadata: HashMap<&str, Value> = match error {
            Conversation(error) => return error.into(),
            Storage(error) => return error.into(),
            NotDir(path) => [
                ("message", "Path is not a directory.".into()),
                ("path", path.to_string_lossy().into()),
            ]
            .into(),
            MissingStorage => [("message", "Missing storage directory".into())].into(),
            MissingHome => [("message", "Missing home directory".into())].into(),
            NotFound(target, id) => [
                ("message", "Not found".into()),
                ("target", target.into()),
                ("id", id.into()),
            ]
            .into(),
            Exists { target, id } => [
                ("message", "Exists".into()),
                ("target", target.into()),
                ("id", id.into()),
            ]
            .into(),
            CannotRemoveActiveConversation(conversation_id) => [
                ("message", "Cannot remove active conversation".into()),
                ("conversation_id", conversation_id.to_string().into()),
            ]
            .into(),
            Id(error) => [
                ("message", "Invalid workspace ID".into()),
                ("error", error.to_string().into()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_conversation::Error> for Error {
    fn from(error: jp_conversation::Error) -> Self {
        use jp_conversation::Error::*;

        let metadata: HashMap<&str, Value> = match error {
            XmlSerialization(se_error) => [
                ("message", "XML serialization error".into()),
                ("error", se_error.to_string().into()),
            ]
            .into(),

            Io(error) => [
                ("message", "IO error".into()),
                ("error", error.to_string().into()),
            ]
            .into(),

            Thread(error) => [
                ("message", "Invalid thread".into()),
                ("error", error.to_string().into()),
            ]
            .into(),

            InvalidIdFormat(error) => [
                ("message", "Invalid ID format".into()),
                ("error", error.to_string().into()),
            ]
            .into(),

            InvalidProviderId(error) => [
                ("message", "Invalid provider ID".into()),
                ("error", error.to_string().into()),
            ]
            .into(),

            Id(error) => return error.into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_storage::Error> for Error {
    fn from(error: jp_storage::Error) -> Self {
        use jp_storage::Error;

        let metadata: HashMap<&str, Value> = match error {
            Error::Conversation(error) => return error.into(),
            Error::Io(error) => return error.into(),
            Error::Json(error) => return error.into(),
            Error::NotDir(path) => [
                ("message", "Path is not a directory.".into()),
                ("path", path.to_string_lossy().into()),
            ]
            .into(),
            Error::NotSymlink(path) => [
                ("message", "Path is not a symlink.".into()),
                ("path", path.to_string_lossy().into()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_id::Error> for Error {
    fn from(error: jp_id::Error) -> Self {
        use jp_id::Error::*;

        let metadata: HashMap<&str, Value> = match error {
            MissingPrefix(prefix) => [
                ("message", "Missing prefix".into()),
                ("prefix", prefix.into()),
            ]
            .into(),
            InvalidPrefix(expected, actual) => [
                ("message", "Invalid prefix".into()),
                ("expected", expected.into()),
                ("actual", actual.into()),
            ]
            .into(),
            MissingVariantAndTargetId => {
                [("message", "Missing variant and target ID".into())].into()
            }
            MissingVariant => [("message", "Missing variant".into())].into(),
            InvalidVariant(variant) => [
                ("message", "Invalid variant".into()),
                ("variant", variant.to_string().into()),
            ]
            .into(),
            UnexpectedVariant(expected, variant) => [
                ("message", "Unexpected variant".into()),
                ("variant", variant.to_string().into()),
                ("expected", expected.to_string().into()),
            ]
            .into(),
            MissingTargetId => [("message", "Missing target ID".into())].into(),
            InvalidTimestamp(timestamp) => [
                ("message", "Invalid timestamp".into()),
                ("timestamp", timestamp.into()),
            ]
            .into(),
            MissingGlobalId => [("message", "Missing global ID".into())].into(),
            InvalidGlobalId(id) => [
                ("message", "Invalid workspace ID".into()),
                ("id", id.into()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}
