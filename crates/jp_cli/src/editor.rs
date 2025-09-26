use std::{fs, path::PathBuf, str::FromStr};

use duct::Expression;
use jp_conversation::{ConversationId, UserMessage};
use time::{macros::format_description, UtcOffset};

use crate::{
    ctx::Ctx,
    error::{Error, Result},
};

/// The name of the file used to store the current query message.
const QUERY_FILENAME: &str = "QUERY_MESSAGE.md";

const CUT_MARKER: &[&str] = &[
    "---------------------------------------8<---------------------------------------",
    "--------------------- EVERYTHING BELOW THIS LINE IS IGNORED --------------------",
    "--------------------------------------->8---------------------------------------",
];

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
}

impl Options {
    pub(crate) fn new(cmd: Expression) -> Self {
        Self {
            cmd,
            cwd: None,
            content: None,
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
    let Options { cmd, cwd, content } = options;

    let exists = path.exists();
    let guard = RevertFileGuard {
        path: Some(path.clone()),
        orig: fs::read_to_string(&path).unwrap_or_default(),
        exists,
    };

    let existing_content = fs::read_to_string(&path).unwrap_or_default();

    if !exists || existing_content.is_empty() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content.unwrap_or_default())?;
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
    ctx: &Ctx,
    conversation_id: &ConversationId,
    initial_message: Option<String>,
    cmd: Expression,
) -> Result<(String, PathBuf)> {
    let root = ctx.workspace.storage_path().unwrap_or(&ctx.workspace.root);
    let history = ctx.workspace.get_messages(conversation_id);

    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

    let mut initial_text = vec![];
    for message in history.iter() {
        let mut buf = String::new();
        buf.push_str("# ");
        buf.push_str(
            &message
                .timestamp
                .to_offset(local_offset)
                .format(&format)
                .unwrap_or_else(|_| message.timestamp.to_string()),
        );
        buf.push_str("\n\n");

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

        buf.push_str("## ASSISTANT\n\n");
        if let Some(reasoning) = &message.reply.reasoning {
            buf.push_str(&comrak::markdown_to_commonmark(
                &format!("### reasoning\n\n{reasoning}\n\n"),
                &options,
            ));
        }
        if let Some(content) = &message.reply.content {
            buf.push_str(&comrak::markdown_to_commonmark(
                &format!(
                    "{}{content}\n\n",
                    if message.reply.reasoning.is_some() {
                        "### response\n\n"
                    } else {
                        ""
                    }
                ),
                &options,
            ));
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
                buf.push_str("## YOU\n\n");
                buf.push_str(&comrak::markdown_to_commonmark(query, &options));
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
        initial_text.push(buf);
    }

    initial_text.push(format!("model: {}\n", ctx.config().assistant.model.id));

    if !initial_text.is_empty() {
        let mut intro = String::new();
        intro.push_str("\n\n");
        intro.push_str(&CUT_MARKER.join("\n"));
        intro.push('\n');
        initial_text.push(intro);
    }

    if let Some(message) = initial_message {
        initial_text.push(message.trim_end().to_owned());
    }

    initial_text.reverse();

    let query_file_path = root.join(QUERY_FILENAME);

    let options = Options::new(cmd)
        .with_cwd(root)
        .with_content(initial_text.join("\n"));
    let (mut content, mut guard) = open(query_file_path.clone(), options)?;

    let eof = CUT_MARKER
        .iter()
        .filter_map(|marker| content.find(marker))
        .min()
        .unwrap_or(content.len());

    content.truncate(eof);
    content = content.trim().to_owned();

    // Disarm the guard, so the file is not reverted.
    guard.disarm();

    Ok((content, query_file_path))
}
