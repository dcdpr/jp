mod attachment;
mod config;
mod conversation;
pub(crate) mod conversation_id;
mod init;
mod lock;
mod query;
pub(crate) mod target;

use std::{fmt, num::NonZeroU8};

use jp_config::PartialAppConfig;
use jp_workspace::Workspace;
use serde_json::Value;
pub(crate) use target::ConversationLoadRequest;

use crate::{Ctx, ctx::IntoPartialAppConfig};

#[derive(Debug, clap::Subcommand)]
#[command(disable_help_subcommand = true)]
#[expect(clippy::large_enum_variant)]
pub(crate) enum Commands {
    /// Initialize a new workspace.
    Init(init::Init),

    /// Configuration management.
    #[command(visible_alias = "cfg")]
    Config(config::Config),

    /// Query the assistant.
    #[command(visible_alias = "q")]
    Query(query::Query),

    /// Manage attachments.
    #[command(visible_alias = "a", alias = "attachments")]
    Attachment(attachment::Attachment),

    // TODO: Remove once we have proper customizable "command aliases".
    #[command(name = "aa", hide = true)]
    AttachmentAdd(attachment::add::Add),

    /// Manage conversations.
    #[command(visible_alias = "c", alias = "conversations")]
    Conversation(conversation::Conversation),
}

impl Commands {
    pub(crate) async fn run(
        self,
        ctx: &mut Ctx,
        handles: Vec<jp_workspace::ConversationHandle>,
    ) -> Output {
        match self {
            Commands::Query(args) => {
                debug_assert!(handles.len() < 2, "Query commands use 0 or 1 handle");
                Box::pin(args.run(ctx, handles.into_iter().next())).await
            }
            Commands::Config(args) => args.run(ctx, handles),
            Commands::Conversation(args) => args.run(ctx, handles).await,
            Commands::Attachment(args) => {
                debug_assert!(handles.is_empty(), "Attachment commands don't use handles");
                args.run(ctx)
            }
            Commands::AttachmentAdd(args) => {
                debug_assert!(handles.is_empty(), "Attachment commands don't use handles");
                args.run(ctx)
            }
            Commands::Init(_) => unreachable!("handled before workspace initialization"),
        }
    }

    /// Declare what conversations this command needs and whether any should
    /// participate in the config loading pipeline.
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        match self {
            Commands::Query(args) => args.conversation_load_request(),
            Commands::Config(args) => args.conversation_load_request(),
            Commands::Conversation(args) => args.conversation_load_request(),
            Commands::Init(_) | Commands::Attachment(_) | Commands::AttachmentAdd(_) => {
                ConversationLoadRequest::none()
            }
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Commands::Query(_) => "query",
            Commands::Config(_) => "config",
            Commands::Attachment(_) => "attachment",
            Commands::AttachmentAdd(_) => "attachment-add",
            Commands::Init(_) => "init",
            Commands::Conversation(_) => "conversation",
        }
    }
}

impl IntoPartialAppConfig for Commands {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Commands::Query(args) => args.apply_cli_config(workspace, partial, merged_config),
            Commands::Attachment(args) => args.apply_cli_config(workspace, partial, merged_config),
            Commands::AttachmentAdd(args) => {
                args.apply_cli_config(workspace, partial, merged_config)
            }
            _ => Ok(partial),
        }
    }

    fn apply_conversation_config(
        &self,
        workspace: &Workspace,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
        handle: &jp_workspace::ConversationHandle,
    ) -> Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Commands::Query(args) => {
                args.apply_conversation_config(workspace, partial, merged_config, handle)
            }
            _ => Ok(partial),
        }
    }
}

pub(crate) type Output = std::result::Result<(), Error>;

#[derive(Debug, thiserror::Error)]
pub(crate) struct Error {
    /// The error code.
    ///
    /// Used to exit the CLI with a specific exit code. This is usually `1`.
    pub(super) code: NonZeroU8,

    /// The optional error message to be displayed to the user.
    pub(super) message: Option<String>,

    /// Metadata to be displayed to the user.
    ///
    /// This is hidden from the user in TTY mode, unless the `--verbose` flag is
    /// set.
    pub(super) metadata: Vec<(String, Value)>,

    /// Whether to disable persistence when this error is encountered.
    pub(super) disable_persistence: bool,
}

impl Error {
    pub(super) fn with_persistence(self, persist: bool) -> Self {
        Self {
            disable_persistence: !persist,
            ..self
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "error {}: {} ({})",
            self.code,
            self.message.as_deref().unwrap_or_default(),
            self.metadata
                .iter()
                .map(|(k, v)| format!("{k}:{v}"))
                .collect::<Vec<_>>()
                .join(", "),
        )
    }
}

impl From<u8> for Error {
    fn from(code: u8) -> Self {
        Self {
            code: code.try_into().unwrap_or(NonZeroU8::new(1).unwrap()),
            message: None,
            metadata: vec![],
            disable_persistence: true,
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

impl From<(u8, String)> for Error {
    fn from((code, message): (u8, String)) -> Self {
        (code, message, vec![]).into()
    }
}

impl From<(u8, &str)> for Error {
    fn from((code, message): (u8, &str)) -> Self {
        (code, message.to_owned()).into()
    }
}

impl From<(u8, String, Vec<(String, Value)>)> for Error {
    fn from((code, message, metadata): (u8, String, Vec<(String, Value)>)) -> Self {
        Self {
            code: code.try_into().unwrap_or(NonZeroU8::new(1).unwrap()),
            message: Some(message),
            metadata: metadata.into_iter().collect(),
            disable_persistence: true,
        }
    }
}

impl From<(u8, &str, Vec<(String, Value)>)> for Error {
    fn from((code, message, metadata): (u8, &str, Vec<(String, Value)>)) -> Self {
        (code, message.to_string(), metadata).into()
    }
}

impl From<Vec<(String, Value)>> for Error {
    fn from(metadata: Vec<(String, Value)>) -> Self {
        (1, metadata).into()
    }
}

impl From<Vec<(&str, Value)>> for Error {
    fn from(metadata: Vec<(&str, Value)>) -> Self {
        metadata
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect::<Vec<_>>()
            .into()
    }
}

impl From<Vec<(&'static str, String)>> for Error {
    fn from(metadata: Vec<(&'static str, String)>) -> Self {
        metadata
            .into_iter()
            .map(|(k, v)| (k, Value::String(v)))
            .collect::<Vec<_>>()
            .into()
    }
}

impl From<(u8, Vec<(String, Value)>)> for Error {
    fn from((code, mut metadata): (u8, Vec<(String, Value)>)) -> Self {
        let message = metadata
            .iter()
            .position(|(k, _)| k == "message")
            .and_then(|i| metadata.remove(i).1.as_str().map(ToString::to_string))
            .unwrap_or_else(|| "Application error".to_owned());

        (code, message, metadata).into()
    }
}

impl From<crate::error::Error> for Error {
    fn from(error: crate::error::Error) -> Self {
        use crate::error::Error::*;

        let metadata: Vec<(&str, String)> = match error {
            Command(error) => return error,
            Config(error) => return error.into(),
            KeyValue(error) => return error.into(),
            Workspace(error) => return error.into(),
            Conversation(error) => return error.into(),
            Mcp(error) => return error.into(),
            Llm(error) => return error.into(),
            Io(error) => return error.into(),
            Url(error) => return error.into(),
            SyntaxHighlight(error) => return error.into(),
            Template(error) => return error.into(),
            Json(error) => return error.into(),
            Toml(error) => return error.into(),
            Which(error) => return error.into(),
            ConfigLoader(error) => return error.into(),
            Tool(error) => return error.into(),
            ModelId(error) => return error.into(),
            Inquire(error) => return error.into(),
            Fmt(error) => return error.into(),
            NotFound(target, id) => [
                ("message", "Not found".into()),
                ("target", target.into()),
                ("id", id),
            ]
            .into(),
            Attachment(error) => [
                ("message", "Attachment error".into()),
                ("error", error.clone()),
            ]
            .into(),
            Editor(error) => [("message", "Editor error".into()), ("error", error.clone())].into(),
            Task(error) => with_cause(error.as_ref(), "Task error"),
            TemplateUndefinedVariable(var) => [
                ("message", "Undefined template variable".to_owned()),
                ("variable", var),
            ]
            .into(),
            MissingEditor => [("message", "Missing editor".to_owned())].into(),
            Schema(error) => [("message", "Invalid schema".to_owned()), ("error", error)].into(),
            MissingStructuredData => {
                [("message", "No structured data in response".to_owned())].into()
            }
            LockTimeout(id) => [
                (
                    "message",
                    format!("Timed out waiting for lock on conversation {id}"),
                ),
                (
                    "suggestion",
                    "Use --no-persist to skip locking, or set $JP_LOCK_DURATION".to_owned(),
                ),
            ]
            .into(),
            NoConversationTarget => [
                (
                    "message",
                    "No conversation targeted. Use one of the following:".to_owned(),
                ),
                (
                    "suggestion",
                    "--id=<id>    target a specific conversation\n--id=last    continue the most \
                     recently active conversation\n--new        start a new \
                     conversation\n$JP_SESSION  set a session identity for automatic tracking"
                        .to_owned(),
                ),
            ]
            .into(),
            CliConfig(error) => {
                [("message", "CLI Config error".to_owned()), ("error", error)].into()
            }
            UnknownModel { model, available } => [
                ("message", "Unknown model".into()),
                ("model", model),
                ("available", available.join(", ")),
            ]
            .into(),
            MissingConfigFile { path, searched } => {
                let mut meta = vec![
                    ("message", "Missing config file".into()),
                    ("path", path.to_string()),
                ];
                if !searched.is_empty() {
                    let dirs = searched
                        .iter()
                        .map(|p| format!("  - {p}"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    meta.push(("searched", format!("Searched in:\n{dirs}")));
                }
                meta
            }
        };

        Self::from(metadata)
    }
}

fn with_cause(
    mut error: &dyn std::error::Error,
    message: impl Into<String>,
) -> Vec<(&'static str, String)> {
    let mut causes = vec![("message", message.into()), ("", format!("{error:#}"))];
    while let Some(cause) = error.source() {
        error = cause;
        causes.push(("", format!("{error:#}")));
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

impl_from_error!(syntect::Error, "Error while formatting code");
impl_from_error!(
    jp_config::assignment::KvAssignmentError,
    "Key-value assignment error"
);
impl_from_error!(jp_config::Error, "Config error");
impl_from_error!(jp_storage::LoadError, "Storage load error");
impl_from_error!(jp_config::ConfigError, "Config error");
impl_from_error!(jp_config::fs::ConfigLoaderError, "Config loader error");
impl_from_error!(jp_conversation::Error, "Conversation error");
impl_from_error!(jp_llm::ToolError, "Tool error");
impl_from_error!(jp_llm::AggregationError, "Tool call aggregation error");
impl_from_error!(jp_mcp::Error, "MCP error");
impl_from_error!(minijinja::Error, "Template error");
impl_from_error!(quick_xml::SeError, "XML serialization error");
impl_from_error!(reqwest::Error, "Error while making HTTP request");
impl_from_error!(serde::de::value::Error, "Deserialization error");
impl_from_error!(serde_json::Error, "Error while parsing JSON");
impl_from_error!(std::io::Error, "IO error");
impl_from_error!(std::num::ParseIntError, "Error parsing integer value");
impl_from_error!(std::str::ParseBoolError, "Error parsing boolean value");
impl_from_error!(toml::de::Error, "Error while parsing TOML");
impl_from_error!(toml::ser::Error, "Error while serializing TOML");
impl_from_error!(url::ParseError, "Error while parsing URL");
impl_from_error!(which::Error, "Which error");
impl_from_error!(jp_config::model::id::ModelIdConfigError, "Model ID error");
impl_from_error!(jp_config::model::id::ModelIdError, "Model ID error");
impl_from_error!(inquire::error::InquireError, "Inquire error");
impl_from_error!(tokio::task::JoinError, "Join error");
impl_from_error!(std::fmt::Error, "fmt error");

impl From<jp_llm::Error> for Error {
    fn from(error: jp_llm::Error) -> Self {
        use jp_llm::Error::*;

        let metadata: Vec<(&str, String)> = match error {
            OpenRouter(error) => return error.into(),
            Conversation(error) => return error.into(),
            XmlSerialization(error) => return error.into(),
            Config(error) => return error.into(),
            Json(error) => return error.into(),
            Request(error) => return error.into(),
            Url(error) => return error.into(),
            ModelIdConfig(error) => return error.into(),
            ModelId(error) => return error.into(),
            ToolCallRequestAggregator(error) => return error.into(),
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
            Gemini(error) => [
                ("message", "Gemini error".into()),
                ("error", error.to_string()),
            ]
            .into(),
            RateLimit { retry_after } => [
                ("message", "Rate limited".into()),
                (
                    "retry_after",
                    retry_after.unwrap_or_default().as_secs().to_string(),
                ),
            ]
            .into(),
            UnknownModel(model) => [("message", "Unknown model".into()), ("model", model)].into(),
            Stream(stream_error) => [
                ("message", "Stream error".into()),
                ("error", stream_error.to_string()),
                ("kind", format!("{:?}", stream_error.kind)),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_openrouter::Error> for Error {
    fn from(error: jp_openrouter::Error) -> Self {
        use jp_openrouter::Error::*;

        let metadata: Vec<(&str, Value)> = match error {
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

impl From<jp_workspace::Error> for Error {
    fn from(error: jp_workspace::Error) -> Self {
        use jp_workspace::Error::*;

        let metadata: Vec<(&str, Value)> = match error {
            Conversation(error) => return error.into(),
            Storage(error) => return error.into(),
            Load(error) => return error.into(),
            Io(error) => return error.into(),
            Config(error) => return error.into(),
            NotDir(path) => [
                ("message", "Path is not a directory.".into()),
                ("path", path.to_string().into()),
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
            Id(error) => [
                ("message", "Invalid workspace ID".into()),
                ("error", error.clone().into()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_storage::Error> for Error {
    fn from(error: jp_storage::Error) -> Self {
        use jp_storage::Error;

        let metadata: Vec<(&str, Value)> = match error {
            Error::Conversation(error) => return error.into(),
            Error::Io(error) => return error.into(),
            Error::Json(error) => return error.into(),
            Error::Toml(error) => return error.into(),
            Error::Config(error) => return error.into(),
            Error::NotDir(path) => [
                ("message", "Path is not a directory.".into()),
                ("path", path.to_string().into()),
            ]
            .into(),
            Error::NotSymlink(path) => [
                ("message", "Path is not a symlink.".into()),
                ("path", path.to_string().into()),
            ]
            .into(),
        };

        Self::from(metadata)
    }
}

impl From<jp_id::Error> for Error {
    fn from(error: jp_id::Error) -> Self {
        use jp_id::Error::*;

        let metadata: Vec<(&str, Value)> = match error {
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
