mod parser;

use std::{
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    path::PathBuf,
    str::FromStr,
};

use duct::Expression;
use itertools::Itertools;
use jp_config::{
    AppConfig, Config as _, PartialAppConfig, ToPartial as _,
    model::parameters::PartialReasoningConfig,
};
use jp_conversation::{ConversationId, UserMessage, message::Messages};
use time::{UtcOffset, macros::format_description};

use crate::{
    ctx::Ctx,
    editor::parser::QueryDocument,
    error::{Error, Result},
};

/// The name of the file used to store the current query message.
const QUERY_FILENAME: &str = "QUERY_MESSAGE.md";

/// How to edit the query.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) enum Editor {
    /// Use whatever editor is configured.
    #[default]
    Default,

    /// Use the given command.
    Command(String),

    /// Do not edit the query.
    Disabled,
}

impl FromStr for Editor {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "true" => Ok(Self::Default),
            "false" => Ok(Self::Disabled),
            s => Ok(Self::Command(s.to_owned())),
        }
    }
}

/// Options for opening an editor.
#[derive(Debug)]
pub(crate) struct Options {
    cmd: Expression,

    /// The working directory to use.
    cwd: Option<PathBuf>,

    /// The initial content to use.
    content: Option<String>,

    /// Whether to force write the file, even if it already exists.
    force_write: bool,
}

impl Options {
    pub(crate) fn new(cmd: Expression) -> Self {
        Self {
            cmd,
            cwd: None,
            content: None,
            force_write: false,
        }
    }

    /// Add a working directory to the editor options.
    #[must_use]
    pub(crate) fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add content to the editor options.
    #[must_use]
    pub(crate) fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// Force write the file, even if it already exists.
    #[must_use]
    pub(crate) fn with_force_write(mut self, force_write: bool) -> Self {
        self.force_write = force_write;
        self
    }
}

pub(crate) struct RevertFileGuard {
    path: Option<PathBuf>,
    orig: String,
    exists: bool,
}

impl RevertFileGuard {
    pub(crate) fn disarm(&mut self) {
        self.path.take();
    }
}

impl Drop for RevertFileGuard {
    fn drop(&mut self) {
        // No path, means this guard was disarmed.
        let Some(path) = &self.path else {
            return;
        };

        // File did not exist, so we remove it, and any empty parent
        // directories.
        if !self.exists {
            let _rm = fs::remove_file(path);
            let mut path = path.clone();
            loop {
                let Some(parent) = path.parent() else {
                    break;
                };

                let Ok(mut dir) = fs::read_dir(parent) else {
                    break;
                };

                if dir.next().is_some() {
                    break;
                }

                let _rm = fs::remove_dir(parent);
                path = parent.to_owned();
            }

            return;
        }

        // File existed, so we restore the original content.
        let _write = fs::write(path, &self.orig);
    }
}

/// Open an editor for the given file with the given content.
///
/// If the file exists, it will be opened, but the content will not be modified
/// (in other words, `content` is ignored).
///
/// When the editor is closed, the contents are returned.
pub(crate) fn open(path: PathBuf, options: Options) -> Result<(String, RevertFileGuard)> {
    let Options {
        cmd,
        cwd,
        content,
        force_write,
    } = options;

    let exists = path.exists();
    let guard = RevertFileGuard {
        path: Some(path.clone()),
        orig: fs::read_to_string(&path).unwrap_or_default(),
        exists,
    };

    let existing_content = fs::read_to_string(&path).unwrap_or_default();

    if !exists || existing_content.is_empty() || force_write {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        let mut current_content = String::new();
        file.read_to_string(&mut current_content)?;

        file.write_all(content.unwrap_or_default().as_bytes())?;
        file.write_all(current_content.as_bytes())?;
    }

    // Open the editor
    let output = cmd
        .before_spawn({
            let path = path.clone();
            move |cmd| {
                cmd.arg(path.clone());

                if let Some(cwd) = &cwd {
                    cmd.current_dir(cwd);
                }

                Ok(())
            }
        })
        .unchecked()
        .run()?;

    let status = output.status;
    if !status.success() {
        return Err(Error::Editor(format!("Editor exited with error: {status}")));
    }

    // Read the edited content
    let content = fs::read_to_string(path)?;

    Ok((content, guard))
}

/// Open an editor for the user to input or edit text using a file in the workspace
pub(crate) fn edit_query(
    ctx: &mut Ctx,
    conversation_id: &ConversationId,
    query: Option<&str>,
    cmd: Expression,
    config_error: Option<&str>,
) -> Result<(String, PathBuf, PartialAppConfig)> {
    let root = ctx.workspace.storage_path().unwrap_or(&ctx.workspace.root);
    let history = ctx.workspace.get_messages(conversation_id).to_messages();
    let query_file_path = root.join(QUERY_FILENAME);

    let existing_content = fs::read_to_string(&query_file_path).unwrap_or_default();
    let mut doc = QueryDocument::try_from(existing_content.as_str()).unwrap_or_default();

    if let Some(v) = query
        && doc.query.is_empty()
    {
        doc.query = v;
    }

    let config_value = build_config_text(ctx.config());
    if doc.meta.config.value.is_empty() {
        doc.meta.config.value = &config_value;
    }

    if let Some(error) = config_error {
        doc.meta.config.error = Some(error);
    }

    let history_value = build_history_text(history);
    doc.meta.history.value = &history_value;

    let options = Options::new(cmd.clone())
        .with_cwd(root)
        .with_content(doc)
        .with_force_write(true);

    let (content, mut guard) = open(query_file_path.clone(), options)?;

    let doc = QueryDocument::try_from(content.as_str()).unwrap_or_default();
    let mut config = PartialAppConfig::empty();
    if !doc.meta.config.value.is_empty() {
        match toml::from_str::<PartialAppConfig>(doc.meta.config.value) {
            Ok(v) => config = v,
            Err(error) => {
                let error = error.to_string();
                return edit_query(ctx, conversation_id, None, cmd, Some(&error));
            }
        }
    }

    guard.disarm();
    Ok((doc.query.to_owned(), query_file_path, config))
}

fn build_config_text(config: &AppConfig) -> String {
    let model_id = &config.assistant.model.id;
    let mut tools = config
        .conversation
        .tools
        .iter()
        .filter_map(|(k, cfg)| cfg.enable().then_some(k))
        .sorted()
        .collect::<Vec<_>>()
        .join(", ");

    if tools.is_empty() {
        tools = "(none)".to_owned();
    }

    let mut active_config = PartialAppConfig::empty();
    active_config.assistant.model.id = model_id.to_partial();
    active_config.assistant.model.parameters.reasoning = config
        .assistant
        .model
        .parameters
        .reasoning
        .map(|v| v.to_partial())
        .or(Some(PartialReasoningConfig::Auto));

    toml::to_string_pretty(&active_config).unwrap_or_default()
}

fn build_history_text(mut history: Messages) -> String {
    let mut text = String::new();

    if !history.is_empty() {
        text.push_str("\n# Conversation History");
    }

    let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");

    let mut messages_with_config = vec![];
    loop {
        let partial = history.config();
        let config = AppConfig::from_partial(partial).ok();
        let Some(message) = history.pop() else {
            break;
        };

        messages_with_config.push((message, config));
    }

    let mut messages = vec![];
    for (message, config) in messages_with_config {
        let mut buf = String::new();
        let timestamp = message
            .timestamp
            .to_offset(local_offset)
            .format(&format)
            .unwrap_or_else(|_| message.timestamp.to_string());

        let options = comrak::Options {
            render: comrak::RenderOptions {
                width: 80,
                unsafe_: true,
                prefer_fenced: true,
                experimental_minimize_commonmark: true,
                ..Default::default()
            },
            ..Default::default()
        };

        buf.push_str("\n\n## Assistant");

        if let Some(cfg) = config {
            buf.push_str(&format!(" ({})", cfg.assistant.model.id));
        }

        buf.push_str(&format!(" on {timestamp}"));

        if let Some(reasoning) = &message.reply.reasoning {
            buf.push_str(&comrak::markdown_to_commonmark(
                &format!("\n\n### reasoning\n\n{reasoning}"),
                &options,
            ));
        }

        if let Some(content) = &message.reply.content {
            buf.push_str("\n\n");
            buf.push_str(
                comrak::markdown_to_commonmark(
                    &format!(
                        "{}{content}",
                        if message.reply.reasoning.is_some() {
                            "### response\n\n"
                        } else {
                            ""
                        }
                    ),
                    &options,
                )
                .trim(),
            );
        }

        for tool_call in &message.reply.tool_calls {
            let Ok(result) = serde_json::to_string_pretty(&tool_call) else {
                continue;
            };

            buf.push_str("## TOOL CALL REQUEST\n\n");
            buf.push_str("```json\n");
            buf.push_str(&result);
            buf.push_str("\n```");
        }

        buf.push_str("\n\n");
        match &message.message {
            UserMessage::Query(query) => {
                buf.push_str("## You\n\n");
                buf.push_str(comrak::markdown_to_commonmark(query, &options).trim());
            }
            UserMessage::ToolCallResults(results) => {
                for result in results {
                    buf.push_str("## TOOL CALL RESULT\n\n");
                    buf.push_str("```\n");
                    buf.push_str(&result.content);
                    buf.push_str("\n```");
                }
            }
        }

        buf.push('\n');
        messages.push(buf);
    }

    messages.reverse();
    text.extend(messages);
    text
}
