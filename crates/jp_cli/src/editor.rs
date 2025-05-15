use std::{
    env, fs,
    path::{Path, PathBuf},
};

use jp_conversation::{MessagePair, UserMessage};
use time::{macros::format_description, UtcOffset};

use crate::{
    error::{Error, Result},
    DEFAULT_VARIABLE_PREFIX,
};

/// The name of the file used to store the current query message.
const QUERY_FILENAME: &str = "QUERY_MESSAGE.md";

const CUT_MARKER: &[&str] = &[
    "---------------------------------------8<---------------------------------------",
    "--------------------- EVERYTHING BELOW THIS LINE IS IGNORED --------------------",
    "--------------------------------------->8---------------------------------------",
];

/// Options for opening an editor.
#[derive(Debug, Default)]
pub struct Options {
    /// The editor command to use.
    ///
    /// If not specified, the `VISUAL` or `EDITOR` environment variables will be
    /// used, in that order.
    pub cmd: Option<String>,

    /// The working directory to use.
    pub cwd: Option<PathBuf>,

    /// The initial content to use.
    pub content: Option<String>,
}

impl Options {
    /// Add a command to the editor options.
    #[must_use]
    #[expect(dead_code)]
    pub fn with_cmd(mut self, cmd: impl Into<String>) -> Self {
        self.cmd = Some(cmd.into());
        self
    }

    /// Add a working directory to the editor options.
    #[must_use]
    pub fn with_cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Add content to the editor options.
    #[must_use]
    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(content.into());
        self
    }
}

pub struct RevertFileGuard {
    path: Option<PathBuf>,
    orig: String,
    exists: bool,
}

impl RevertFileGuard {
    pub fn disarm(&mut self) {
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
pub fn open(path: impl AsRef<Path>, options: Options) -> Result<(String, RevertFileGuard)> {
    let Options { cmd, cwd, content } = options;

    let path = path.as_ref();
    let exists = path.exists();
    let guard = RevertFileGuard {
        path: Some(path.to_owned()),
        orig: fs::read_to_string(path).unwrap_or_default(),
        exists,
    };

    if !exists {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content.unwrap_or_default())?;
    }

    let editor_cmd = cmd
        .ok_or("Undefined")
        .or_else(|_| env::var(format!("{DEFAULT_VARIABLE_PREFIX}_EDITOR")))
        .or_else(|_| env::var("VISUAL"))
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string()); // TODO: Check different editors
                                               // (neovim, vim, vi, emacs, ...)

    // Open the editor
    let mut cmd = std::process::Command::new(&editor_cmd);
    cmd.arg(path);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err(Error::Editor(format!("Editor exited with error: {status}")));
    }

    // Read the edited content
    let content = fs::read_to_string(path)?;

    Ok((content, guard))
}

/// Open an editor for the user to input or edit text using a file in the workspace
pub fn edit_query(
    root: &Path,
    initial_message: Option<String>,
    history: &[MessagePair],
) -> Result<(String, PathBuf)> {
    let format = format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
    let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

    let mut initial_text = vec![];
    for message in history {
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

        buf.push_str("## ASSISTANT\n\n");
        if let Some(reasoning) = &message.reply.reasoning {
            buf.push_str(&comrak::markdown_to_commonmark(
                &format!("> **reasoning**\n> {reasoning}\n\n"),
                &options,
            ));
        }
        if let Some(content) = &message.reply.content {
            buf.push_str(&comrak::markdown_to_commonmark(
                &format!("{content}\n\n"),
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

        initial_text.push(buf);
    }

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

    let options = Options::default()
        .with_cwd(root)
        .with_content(initial_text.join("\n"));
    let (mut content, mut guard) = open(&query_file_path, options)?;

    let eof = CUT_MARKER
        .iter()
        .filter_map(|marker| content.find(marker))
        .min()
        .unwrap_or(content.len());

    content.truncate(eof);

    // Disarm the guard, so the file is not reverted.
    guard.disarm();

    Ok((content, query_file_path))
}
