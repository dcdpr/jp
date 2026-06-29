mod parser;

use std::{
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    sync::Arc,
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{FixedOffset, Local};
use jp_config::{
    AppConfig, PartialAppConfig, ToPartial as _, editor::EditorConfig,
    model::parameters::PartialReasoningConfig,
};
use jp_conversation::{
    ConversationStream,
    event::{ChatResponse, EventKind},
};
use jp_editor::{EditOutcome, EditRequest, EditorBackend, TerminalEditorBackend};

use crate::{
    editor::parser::QueryDocument,
    error::{Error, Result},
};

/// Build a terminal editor backend from the resolved editor configuration.
///
/// Returns `None` when no editor command resolves: neither `editor.cmd` is set
/// nor does a configured editor environment variable point at an installed
/// binary.
pub(crate) fn build_editor_backend(config: &EditorConfig) -> Option<Arc<dyn EditorBackend>> {
    config
        .command()
        .map(|cmd| Arc::new(TerminalEditorBackend::new(cmd)) as Arc<dyn EditorBackend>)
}

/// The name of the file used to store the current query message.
pub(crate) const QUERY_FILENAME: &str = "QUERY_MESSAGE.md";

/// Options for opening an editor.
#[derive(Debug)]
pub(crate) struct Options {
    /// The working directory to use.
    cwd: Option<Utf8PathBuf>,

    /// The initial content to use.
    content: Option<String>,

    /// Whether to force write the file, even if it already exists.
    force_write: bool,
}

impl Options {
    pub(crate) fn new() -> Self {
        Self {
            cwd: None,
            content: None,
            force_write: false,
        }
    }

    /// Add a working directory to the editor options.
    #[must_use]
    pub(crate) fn with_cwd(mut self, cwd: impl Into<Utf8PathBuf>) -> Self {
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
    path: Option<Utf8PathBuf>,
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
            while let Some(parent) = path.parent() {
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
/// When the editor is closed, the interaction outcome and the file's contents
/// are returned.
/// On [`EditOutcome::Cancelled`] the contents reflect whatever the editor left
/// on disk; the caller decides how to treat a cancellation.
pub(crate) fn open(
    path: Utf8PathBuf,
    options: Options,
    editor: &dyn EditorBackend,
) -> Result<(EditOutcome, String, RevertFileGuard)> {
    let Options {
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

    let outcome = editor
        .edit_file(EditRequest {
            paths: std::slice::from_ref(&path),
            cwd: cwd.as_deref(),
        })
        .map_err(|error| Error::Editor(error.to_string()))?;

    // Read the edited content
    let content = fs::read_to_string(path)?;

    Ok((outcome, content, guard))
}

/// Open an editor for the user to input or edit text using a file in the
/// workspace
pub(crate) fn edit_query(
    config: &AppConfig,
    conversation_root: &Utf8Path,
    stream: &ConversationStream,
    query: &str,
    editor: &dyn EditorBackend,
    config_error: Option<&str>,
) -> Result<(String, PartialAppConfig)> {
    let query_file_path = conversation_root.join(QUERY_FILENAME);
    let existing_content = fs::read_to_string(&query_file_path).unwrap_or_default();
    let mut doc = QueryDocument::try_from(existing_content.as_str()).unwrap_or_default();

    if doc.query.is_empty() {
        doc.query = query;
    }

    let config_value = build_config_text(config);
    if doc.meta.config.value.is_empty() {
        doc.meta.config.value = &config_value;
    }

    if let Some(error) = config_error {
        doc.meta.config.error = Some(error);
    }

    let history_value = build_history_text(stream);
    doc.meta.history.value = &history_value;

    let options = Options::new()
        .with_cwd(conversation_root)
        .with_content(doc)
        .with_force_write(true);

    let (outcome, content, mut guard) = open(query_file_path.clone(), options, editor)?;

    // A cancelled editor (non-zero exit) sends nothing: return an empty query so
    // the caller skips it, and let the guard revert `QUERY_MESSAGE.md`.
    if outcome == EditOutcome::Cancelled {
        return Ok((String::new(), PartialAppConfig::empty()));
    }

    let doc = QueryDocument::try_from(content.as_str()).unwrap_or_default();
    let mut partial = PartialAppConfig::empty();
    if !doc.meta.config.value.is_empty() {
        match toml::from_str::<PartialAppConfig>(doc.meta.config.value) {
            Ok(v) => partial = v,
            Err(error) => {
                let error = error.to_string();
                return edit_query(config, conversation_root, stream, "", editor, Some(&error));
            }
        }
    }

    guard.disarm();
    Ok((doc.query.to_owned(), partial))
}

fn build_config_text(config: &AppConfig) -> String {
    let model_id = &config.assistant.model.id;
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

fn build_history_text(history: &ConversationStream) -> String {
    let mut text = String::new();

    if !history.is_empty() {
        text.push_str("\n# Conversation History (last 10 entries)");
    }

    let local_offset: FixedOffset = *Local::now().offset();
    let format = "%Y-%m-%d %H:%M:%S";

    let mut messages = vec![];
    for event in history.iter().rev().take(10) {
        let mut buf = String::new();
        let timestamp = event
            .timestamp
            .with_timezone(&local_offset)
            .format(format)
            .to_string();

        let options = comrak::Options {
            render: comrak::options::Render {
                width: 80,
                r#unsafe: true,
                prefer_fenced: true,
                experimental_minimize_commonmark: true,
                ..Default::default()
            },
            ..Default::default()
        };

        match &event.kind {
            EventKind::ChatRequest(request) => {
                buf.push_str(&format!("## You on {timestamp}\n\n"));
                buf.push_str(comrak::markdown_to_commonmark(&request.content, &options).trim());
            }
            EventKind::ChatResponse(response) => match response {
                ChatResponse::Message { message } => {
                    buf.push_str("\n\n## Assistant");
                    buf.push_str(&format!(" ({})", event.config.assistant.model.id));
                    buf.push_str(&format!(" on {timestamp}\n\n"));
                    buf.push_str(comrak::markdown_to_commonmark(message, &options).trim());
                }
                ChatResponse::Reasoning { reasoning, .. } => {
                    buf.push_str("\n\n## Assistant (reasoning)");
                    buf.push_str(&format!(" ({})", event.config.assistant.model.id));
                    buf.push_str(&format!(" on {timestamp}\n\n"));
                    buf.push_str(comrak::markdown_to_commonmark(reasoning, &options).trim());
                }
                ChatResponse::Structured { data } => {
                    buf.push_str("\n\n## Assistant (structured)");
                    buf.push_str(&format!(" ({})", event.config.assistant.model.id));
                    buf.push_str(&format!(" on {timestamp}\n\n"));
                    buf.push_str("```json\n");
                    if let Ok(pretty) = serde_json::to_string_pretty(data) {
                        buf.push_str(&pretty);
                    } else {
                        buf.push_str(&data.to_string());
                    }
                    buf.push_str("\n```");
                }
            },
            EventKind::ToolCallRequest(request) => {
                if let Ok(json) = serde_json::to_string_pretty(request) {
                    buf.push_str(&format!("\n\n## Tool Call Request on {timestamp}\n\n"));
                    buf.push_str("```json\n");
                    buf.push_str(&json);
                    buf.push_str("\n```");
                }
            }
            EventKind::ToolCallResponse(response) => {
                if response.result.is_ok() {
                    buf.push_str(&format!("\n\n## Tool Call Result on {timestamp}\n\n"));
                } else {
                    buf.push_str(&format!("\n\n## Tool Call **Error** on {timestamp}\n\n"));
                }
                buf.push_str("```\n");
                buf.push_str(&response.result.clone().unwrap_or_else(|err| err));
                buf.push_str("\n```");
            }
            EventKind::InquiryRequest(request) => {
                buf.push_str(&format!(
                    "\n\n## Inquiry Request ({:?}) on {timestamp}\n\n",
                    request.source
                ));
                buf.push_str(&request.question.text);
            }
            EventKind::InquiryResponse(response) => {
                buf.push_str(&format!("\n\n## Inquiry Response on {timestamp}\n\n"));
                buf.push_str("Answer: ");
                buf.push_str(&response.answer.to_string());
            }
            EventKind::TurnStart(_) => {}
        }

        buf.push_str("\n\n");
        messages.push(buf);
    }

    text.extend(messages);
    text
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
