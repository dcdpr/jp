use std::{borrow::Cow, fmt::Write as _};

use crossterm::style::Stylize as _;
use jp_conversation::{ConversationId, EventKind, event::ChatResponse};
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::FlagIds},
    ctx::Ctx,
};

/// Maximum number of characters to show from a matching line.
const TRUNCATE_AT: usize = 60;

#[derive(Debug, clap::Args)]
pub(crate) struct Grep {
    /// The search pattern.
    pattern: String,

    #[command(flatten)]
    target: FlagIds<true, true>,

    /// Case-insensitive matching.
    #[arg(long)]
    ignore_case: bool,
}

impl Grep {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_none(&self.target.ids)
    }

    #[expect(clippy::needless_pass_by_value)]
    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let mut pattern = self.pattern;
        if self.ignore_case {
            pattern = pattern.to_lowercase();
        }

        // If handles were provided, search only those. Otherwise search all.
        let ids: Vec<ConversationId> = if handles.is_empty() {
            ctx.workspace.conversations().map(|(id, _)| *id).collect()
        } else {
            handles.iter().map(ConversationHandle::id).collect()
        };

        let mut output = String::new();
        for id in ids {
            let Ok(handle) = ctx.workspace.acquire_conversation(&id) else {
                continue;
            };
            let Ok(events) = ctx.workspace.events(&handle) else {
                continue;
            };

            for event in events.iter() {
                let texts = event_text_content(&event.event.kind);
                for text in texts {
                    for line in text.lines() {
                        let matches = if self.ignore_case {
                            line.to_lowercase().contains(&pattern)
                        } else {
                            line.contains(&pattern)
                        };

                        if !matches {
                            continue;
                        }

                        let truncated = truncate_line(line, TRUNCATE_AT);
                        let _ = writeln!(
                            output,
                            "{}: {}",
                            id.to_string().bold().yellow(),
                            truncated.dim()
                        );
                    }
                }
            }
        }

        if output.ends_with('\n') {
            output.pop();
        }

        if output.is_empty() {
            return Err("No matches found.".into());
        }

        ctx.printer.println(&output);
        Ok(())
    }
}

/// Extract all searchable text content from an event.
fn event_text_content(kind: &EventKind) -> Vec<Cow<'_, str>> {
    match kind {
        EventKind::ChatRequest(req) => vec![req.content.as_str().into()],
        EventKind::ChatResponse(ChatResponse::Message { message }) => vec![message.as_str().into()],
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => {
            vec![reasoning.as_str().into()]
        }
        EventKind::ToolCallRequest(req) => vec![req.name.as_str().into()],
        EventKind::ToolCallResponse(resp) => vec![resp.content().into()],
        EventKind::InquiryRequest(req) => vec![req.question.text.as_str().into()],
        EventKind::ChatResponse(ChatResponse::Structured { data }) => vec![data.to_string().into()],
        EventKind::InquiryResponse(_) | EventKind::TurnStart(_) => vec![],
    }
}

/// Truncate a line to `max` characters, appending `...` if truncated.
fn truncate_line(line: &str, max: usize) -> String {
    let trimmed = line.trim();
    if trimmed.len() <= max {
        return trimmed.to_owned();
    }

    // Find a char boundary at or before `max`.
    let end = trimmed
        .char_indices()
        .take_while(|(i, _)| *i <= max)
        .last()
        .map_or(max, |(i, _)| i);

    format!("{}...", &trimmed[..end])
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod tests;
