use std::{env, fs, path::Path};

use jp_conversation::{MessagePair, UserMessage};
use time::{macros::format_description, UtcOffset};
use tracing::trace;

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

/// Open an editor for the user to input or edit text using a file in the workspace
pub fn open_editor(
    root: &Path,
    initial_message: Option<String>,
    history: &[MessagePair],
) -> Result<String> {
    let editor_cmd = env::var(format!("{DEFAULT_VARIABLE_PREFIX}_EDITOR"))
        .or_else(|_| env::var("VISUAL"))
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string()); // TODO: Check different editors
                                               // (neovim, vim, vi, emacs, ...)

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

    let file_path = root.join(QUERY_FILENAME);
    if !file_path.exists() {
        fs::write(&file_path, initial_text.join("\n"))?;
    }

    // Open the editor
    let status = std::process::Command::new(&editor_cmd)
        .current_dir(root)
        .arg(&file_path)
        .status()?;

    if !status.success() {
        return Err(Error::Editor(format!("Editor exited with error: {status}")));
    }

    // Read the edited content
    let mut content = fs::read_to_string(&file_path)?;

    let eof = CUT_MARKER
        .iter()
        .filter_map(|marker| content.find(marker))
        .min()
        .unwrap_or(content.len());

    content.truncate(eof);

    Ok(content)
}

/// Remove the query file after successful response
pub fn cleanup_query_file(workspace_root: &Path) -> Result<()> {
    let file_path = workspace_root.join(QUERY_FILENAME);
    if file_path.exists() {
        trace!(path = %file_path.display(), "Removing old query file.");
        fs::remove_file(file_path)?;
    }

    Ok(())
}
