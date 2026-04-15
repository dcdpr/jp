use std::{borrow::Cow, fmt::Write as _};

use chrono::{DateTime, Utc};
use crossterm::style::Stylize as _;
use jp_conversation::{ConversationId, EventKind, event::ChatResponse};
use jp_workspace::ConversationHandle;
use serde_json::json;
use tracing::warn;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::FlagIds},
    ctx::Ctx,
    output::print_json,
};

/// Maximum number of characters to show from a matching line.
const TRUNCATE_AT: usize = 60;

#[derive(Debug, Default, clap::Args)]
pub(crate) struct Grep {
    /// The search pattern.
    pattern: String,

    #[command(flatten)]
    target: FlagIds<true, true>,

    /// Case-insensitive matching.
    #[arg(long)]
    ignore_case: bool,

    /// Number of context lines to show around each match.
    #[arg(long, default_value_t = 0)]
    context: usize,

    /// Sort conversations by a field.
    #[arg(long, value_enum, default_value_t)]
    sort: Sort,

    /// Reverse the sort order (newest/latest first).
    #[arg(long)]
    descending: bool,

    /// Print only the matched (and context) lines, without any decoration.
    #[arg(long)]
    raw: bool,
}

impl Grep {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_none(&self.target)
    }

    #[expect(clippy::needless_pass_by_value)]
    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let pattern = if self.ignore_case {
            self.pattern.to_lowercase()
        } else {
            self.pattern.clone()
        };

        // If handles were provided, search only those. Otherwise search all.
        let mut ids: Vec<_> = if handles.is_empty() {
            ctx.workspace.conversations().map(|(id, _)| *id).collect()
        } else {
            handles.iter().map(ConversationHandle::id).collect()
        };

        self.sort_ids(&mut ids, ctx);

        let hits = self.collect_hits(&ids, &pattern, ctx);

        if hits.is_empty() {
            return Err("No matches found.".into());
        }

        self.render(&hits, ctx);
        Ok(())
    }

    fn collect_hits(&self, ids: &[ConversationId], pattern: &str, ctx: &mut Ctx) -> Vec<Hit> {
        let mut hits = Vec::new();

        for &id in ids {
            let handle = match ctx.workspace.acquire_conversation(&id) {
                Ok(handle) => handle,
                Err(error) => {
                    warn!(%id, %error, "Failed to load conversation");
                    continue;
                }
            };

            let events = match ctx.workspace.events(&handle) {
                Ok(events) => events,
                Err(error) => {
                    warn!(%id, %error, "Failed to load conversation events");
                    continue;
                }
            };

            for lines in events.iter().map(|e| event_lines(&e.event.kind)) {
                let match_indices = matching_lines(&lines, pattern, self.ignore_case);
                if match_indices.is_empty() {
                    continue;
                }

                let ranges = context_ranges(&match_indices, self.context, lines.len());
                for (range_idx, (start, end)) in ranges.iter().enumerate() {
                    for (i, line) in lines.iter().enumerate().skip(*start).take(end - start + 1) {
                        hits.push(Hit {
                            id,
                            text: (*line).to_owned(),
                            is_match: match_indices.contains(&i),
                            group_break: range_idx > 0 && i == *start,
                        });
                    }
                }
            }
        }

        hits
    }

    fn render(&self, hits: &[Hit], ctx: &Ctx) {
        let format = ctx.printer.format();

        if format.is_json() {
            Self::render_json(hits, ctx);
        } else if self.raw {
            Self::render_raw(hits, ctx);
        } else {
            Self::render_text(hits, ctx);
        }
    }

    fn render_text(hits: &[Hit], ctx: &Ctx) {
        let pretty = ctx.printer.pretty_printing_enabled();
        let mut output = String::new();

        for hit in hits {
            if hit.group_break {
                if pretty {
                    let _ = writeln!(output, "{}", "--".dim());
                } else {
                    let _ = writeln!(output, "--");
                }
            }

            let truncated = truncate_line(&hit.text, TRUNCATE_AT);
            let sep = if hit.is_match { ":" } else { "-" };
            let id_str = hit.id.to_string();

            if pretty {
                let _ = writeln!(
                    output,
                    "{}{sep} {}",
                    id_str.bold().yellow(),
                    truncated.dim()
                );
            } else {
                let _ = writeln!(output, "{id_str}{sep} {truncated}");
            }
        }

        let output = output.trim_end_matches('\n');
        ctx.printer.println_raw(output);
    }

    fn render_raw(hits: &[Hit], ctx: &Ctx) {
        let mut output = String::new();

        for hit in hits {
            if hit.group_break {
                let _ = writeln!(output, "--");
            }
            let _ = writeln!(output, "{}", hit.text.trim());
        }

        let output = output.trim_end_matches('\n');
        ctx.printer.println_raw(output);
    }

    fn render_json(hits: &[Hit], ctx: &Ctx) {
        let entries: Vec<_> = hits
            .iter()
            .map(|hit| {
                json!({
                    "id": hit.id.to_string(),
                    "text": hit.text.trim(),
                    "match": hit.is_match,
                })
            })
            .collect();

        print_json(&ctx.printer, &json!(entries));
    }

    fn sort_ids(&self, ids: &mut [ConversationId], ctx: &Ctx) {
        ids.sort_by(|a, b| {
            let ord = match self.sort {
                Sort::Created => a.timestamp().cmp(&b.timestamp()),
                Sort::Activated => {
                    let ts = |id| -> DateTime<Utc> {
                        ctx.workspace
                            .acquire_conversation(id)
                            .ok()
                            .and_then(|h| ctx.workspace.metadata(&h).ok())
                            .map(|m| m.last_activated_at)
                            .unwrap_or_default()
                    };

                    ts(a).cmp(&ts(b))
                }
                Sort::Updated => {
                    let ts = |id| -> Option<DateTime<Utc>> {
                        ctx.workspace
                            .acquire_conversation(id)
                            .ok()
                            .and_then(|h| ctx.workspace.metadata(&h).ok())
                            .and_then(|m| m.last_event_at)
                    };

                    ts(a).cmp(&ts(b))
                }
            };

            if self.descending { ord.reverse() } else { ord }
        });
    }
}

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
enum Sort {
    /// Sort by creation time (conversation ID).
    #[default]
    Created,

    /// Sort by last activation time.
    Activated,

    /// Sort by last event time.
    Updated,
}

/// A single output line from a grep search.
struct Hit {
    /// The target conversation ID.
    id: ConversationId,

    /// The line text.
    text: String,

    /// If false, this is a "context" line.
    is_match: bool,

    /// Whether a `--` group separator should precede this line.
    group_break: bool,
}

/// Return indices of lines that match the pattern.
fn matching_lines(lines: &[&str], pattern: &str, ignore_case: bool) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            if ignore_case {
                line.to_lowercase().contains(pattern)
            } else {
                line.contains(pattern)
            }
        })
        .map(|(i, _)| i)
        .collect()
}

/// Build merged, non-overlapping `(start, end)` ranges around each match index,
/// expanded by `ctx` lines in both directions, clamped to `[0, count)`.
fn context_ranges(indices: &[usize], ctx: usize, count: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &idx in indices {
        let start = idx.saturating_sub(ctx);
        let end = (idx + ctx).min(count - 1);
        if let Some(last) = ranges.last_mut() {
            // Merge with previous range if overlapping or adjacent.
            if start <= last.1 + 1 {
                last.1 = last.1.max(end);
                continue;
            }
        }

        ranges.push((start, end));
    }

    ranges
}

/// Extract all searchable text content from an event.
fn event_lines(kind: &EventKind) -> Vec<&str> {
    match kind {
        EventKind::ChatRequest(req) => req.content.lines().collect(),
        EventKind::ChatResponse(ChatResponse::Message { message }) => message.lines().collect(),
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => {
            reasoning.lines().collect()
        }
        EventKind::ToolCallRequest(req) => req.name.lines().collect(),
        EventKind::ToolCallResponse(resp) => resp.content().lines().collect(),
        EventKind::InquiryRequest(req) => req.question.text.lines().collect(),
        EventKind::ChatResponse(ChatResponse::Structured { data }) => {
            data.as_str().iter().flat_map(|text| text.lines()).collect()
        }
        EventKind::InquiryResponse(_) | EventKind::TurnStart(_) => vec![],
    }
}

/// Truncate a line to `max` characters, appending `...` if truncated.
fn truncate_line(line: &str, max: usize) -> Cow<'_, str> {
    let trimmed = line.trim();
    if trimmed.len() <= max {
        return trimmed.into();
    }

    // Find a char boundary at or before `max`.
    let end = trimmed
        .char_indices()
        .take_while(|(i, _)| *i <= max)
        .last()
        .map_or(max, |(i, _)| i);

    format!("{}...", &trimmed[..end]).into()
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod tests;
