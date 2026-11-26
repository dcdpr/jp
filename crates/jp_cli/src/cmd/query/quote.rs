//! `--quote`: seeding a request with the assistant's last message, for an
//! inline reply.

use jp_config::AppConfig;
use jp_conversation::{ConversationEvent, ConversationStream, event::ChatRequest};
use jp_md::format::Formatter;
use strip_ansi_escapes::strip_str;

/// Return the most recent assistant message text in the stream.
///
/// Walks the stream in reverse and returns the first `ChatResponse::Message` it
/// encounters.
/// Reasoning, structured-data responses, and tool calls are skipped.
fn last_assistant_message(stream: &ConversationStream) -> Option<&str> {
    stream
        .iter()
        .rev()
        .filter_map(ConversationEvent::as_chat_response)
        .find_map(|r| r.as_message())
}

/// Prefix each line of `text` with `> ` for use as a markdown blockquote.
///
/// Empty lines are emitted as just `>` (no trailing space) so the blockquote
/// stays visually continuous across paragraph breaks while avoiding
/// trailing-whitespace warnings in editors.
fn blockquote(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prepend the stream's last assistant message to `request` as a quoted reply
/// seed.
///
/// The message is reformatted with `style.markdown`'s wrap width and table
/// settings before quoting, so a table copied out of a prior response stays
/// aligned and long paragraphs stay wrapped.
/// With `prefixed` the reformatted message is marked up as a markdown
/// blockquote, otherwise it is inserted verbatim.
/// Returns `false` when the stream holds no assistant message, leaving
/// `request` untouched.
pub(super) fn seed_quoted_reply(
    request: &mut ChatRequest,
    stream: &ConversationStream,
    prefixed: bool,
    config: &AppConfig,
) -> bool {
    let Some(message) = last_assistant_message(stream) else {
        return false;
    };

    let formatted = reformat_quoted_message(message, config);
    let quoted = if prefixed {
        blockquote(&formatted)
    } else {
        formatted
    };
    request.content = format!("{quoted}\n\n{request}");

    true
}

/// Reformat `message` with the project's standard markdown formatting: prose
/// wrapped at `style.markdown.wrap_width`, tables padded into aligned columns.
///
/// The result is markdown source rather than terminal output, so the display-
/// only parts of that formatting are left off: styling escapes are stripped,
/// horizontal rules stay the plain `---` a parser reads back as one, and table
/// columns keep their full content.
/// `style.markdown.table_max_column_width` deliberately does not apply here: it
/// cuts a wide header short and splits a wide cell across physical lines, which
/// reads fine on screen but puts truncated text and extra rows into the request
/// the user sends next.
fn reformat_quoted_message(message: &str, config: &AppConfig) -> String {
    let formatter = Formatter::with_width(config.style.markdown.wrap_width)
        .table_max_column_width(0)
        .pretty_hr(false);

    let rendered = formatter
        .format_terminal(message)
        .unwrap_or_else(|_| message.to_owned());

    strip_str(rendered).trim_end().to_owned()
}

#[cfg(test)]
#[path = "quote_tests.rs"]
mod tests;
