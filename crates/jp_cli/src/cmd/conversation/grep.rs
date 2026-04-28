use std::{
    borrow::Cow,
    collections::HashSet,
    fmt::{self, Write as _},
};

use chrono::{DateTime, Utc};
use crossterm::style::Stylize as _;
use jp_conversation::{ConversationId, EventKind, event::ChatResponse};
use jp_workspace::ConversationHandle;
use rayon::prelude::*;
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

    /// Restrict the search to specific parts of the conversation.
    ///
    /// Repeatable or comma-separated. If omitted, every part is searched.
    /// Meta-scopes `chat` and `tool` expand to their concrete members.
    #[arg(long = "scope", short = 's', value_enum, value_delimiter = ',', num_args = 1..)]
    scopes: Vec<Scope>,
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

        let wanted = expand_scopes(&self.scopes);

        // If handles were provided, search only those. Otherwise search all.
        let mut ids: Vec<_> = if handles.is_empty() {
            ctx.workspace.conversations().map(|(id, _)| *id).collect()
        } else {
            handles.iter().map(ConversationHandle::id).collect()
        };

        self.sort_ids(&mut ids, ctx);

        let hits = self.collect_hits(&ids, &pattern, &wanted, ctx);

        if hits.is_empty() {
            return Err("No matches found.".into());
        }

        self.render(&hits, ctx);
        Ok(())
    }

    fn collect_hits(
        &self,
        ids: &[ConversationId],
        pattern: &str,
        wanted: &HashSet<ConcreteScope>,
        ctx: &Ctx,
    ) -> Vec<Hit> {
        // Any scope other than `Title` is sourced from the event stream.
        // Skipping the event pass entirely when it can't contribute avoids a
        // sequential disk read per conversation.
        let needs_events = needs_events_for(wanted);

        // Each worker produces an independent `Vec<Hit>`. Collecting via
        // `Vec<Vec<Hit>>` preserves input order (rayon `collect` is
        // order-preserving, unlike `reduce`); flatten merges per-id results
        // while keeping the (already-sorted) id order intact.
        let per_id: Vec<Vec<Hit>> = ids
            .par_iter()
            .map(|&id| self.collect_hits_for_id(id, pattern, wanted, needs_events, ctx))
            .collect();

        per_id.into_iter().flatten().collect()
    }

    /// Per-conversation hit collection. Pure function over `&Ctx`, so it is
    /// safe to invoke concurrently from a rayon worker.
    fn collect_hits_for_id(
        &self,
        id: ConversationId,
        pattern: &str,
        wanted: &HashSet<ConcreteScope>,
        needs_events: bool,
        ctx: &Ctx,
    ) -> Vec<Hit> {
        let mut hits = Vec::new();

        let handle = match ctx.workspace.acquire_conversation(&id) {
            Ok(handle) => handle,
            Err(error) => {
                warn!(%id, %error, "Failed to load conversation");
                return hits;
            }
        };

        if wanted.contains(&ConcreteScope::Title)
            && let Some(title) = title_for(ctx, &handle)
        {
            let lines: Vec<_> = title.lines().collect();

            collect_scope_hits(
                &mut hits,
                id,
                ConcreteScope::Title,
                &lines,
                pattern,
                self.ignore_case,
                self.context,
            );
        }

        if !needs_events {
            return hits;
        }

        let events = match ctx.workspace.events(&handle) {
            Ok(events) => events,
            Err(error) => {
                warn!(%id, %error, "Failed to load conversation events");
                return hits;
            }
        };

        for event in events.iter() {
            let Some(scope) = event_scope(&event.event.kind) else {
                continue;
            };
            if !wanted.contains(&scope) {
                continue;
            }

            let lines = event_lines(&event.event.kind);
            let line_refs: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();
            collect_scope_hits(
                &mut hits,
                id,
                scope,
                &line_refs,
                pattern,
                self.ignore_case,
                self.context,
            );
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
            self.render_text(hits, ctx);
        }
    }

    fn render_text(&self, hits: &[Hit], ctx: &Ctx) {
        let pretty = ctx.printer.pretty_printing_enabled();

        // Show the scope column only when the user explicitly asked for more
        // than one scope. A bare `jp c grep foo` or `--scope title` stays
        // visually identical to the pre-scope output.
        let show_scope = self.scopes.len() > 1;
        let scope_width = if show_scope {
            hits.iter()
                .map(|h| h.scope.as_str().len())
                .max()
                .unwrap_or(0)
        } else {
            0
        };

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

            if show_scope {
                let scope_str = hit.scope.as_str();
                let pad = scope_width.saturating_sub(scope_str.len());
                if pretty {
                    let _ = writeln!(
                        output,
                        "{} {:pad$}{}{sep} {}",
                        id_str.bold().yellow(),
                        "",
                        scope_str.blue(),
                        truncated.dim(),
                        pad = pad,
                    );
                } else {
                    let _ = writeln!(
                        output,
                        "{id_str} {:pad$}{scope_str}{sep} {truncated}",
                        "",
                        pad = pad,
                    );
                }
            } else if pretty {
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
                    "scope": hit.scope.as_str(),
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

/// Parts of a conversation that can be restricted with `--scope`.
///
/// Meta-scopes (`all`, `chat`, `tool`) expand to one or more `ConcreteScope`s
/// at search time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum Scope {
    /// Search every part (default when no `--scope` is given).
    All,

    /// Shorthand for `user`, `assistant`, `reasoning`, `structured`.
    Chat,

    /// Shorthand for `tool-call` and `tool-result`.
    Tool,

    /// The conversation title.
    Title,

    /// User chat requests.
    User,

    /// Assistant chat responses (message text).
    Assistant,

    /// Assistant reasoning text.
    Reasoning,

    /// Structured assistant output.
    Structured,

    /// Tool call requests (name and arguments).
    ToolCall,

    /// Tool call results.
    ToolResult,

    /// Inquiry questions.
    Inquiry,
}

/// Leaf scopes — the actual partitioning of conversation content. There is no
/// meta-scope here, so this is what the search pipeline filters against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ConcreteScope {
    Title,
    User,
    Assistant,
    Reasoning,
    Structured,
    ToolCall,
    ToolResult,
    Inquiry,
}

impl ConcreteScope {
    const ALL: [Self; 8] = [
        Self::Title,
        Self::User,
        Self::Assistant,
        Self::Reasoning,
        Self::Structured,
        Self::ToolCall,
        Self::ToolResult,
        Self::Inquiry,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Reasoning => "reasoning",
            Self::Structured => "structured",
            Self::ToolCall => "tool-call",
            Self::ToolResult => "tool-result",
            Self::Inquiry => "inquiry",
        }
    }
}

impl fmt::Display for ConcreteScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether the wanted scope set contains anything sourced from the event
/// stream (i.e. something beyond `Title`).
fn needs_events_for(wanted: &HashSet<ConcreteScope>) -> bool {
    wanted.iter().any(|s| *s != ConcreteScope::Title)
}

/// Expand a user-facing list of scopes to the concrete set the search uses.
///
/// An empty input (no `--scope` flag) behaves as `all`.
fn expand_scopes(scopes: &[Scope]) -> HashSet<ConcreteScope> {
    if scopes.is_empty() {
        return ConcreteScope::ALL.iter().copied().collect();
    }

    let mut out = HashSet::new();
    for scope in scopes {
        match scope {
            Scope::All => out.extend(ConcreteScope::ALL),
            Scope::Chat => {
                out.extend([
                    ConcreteScope::User,
                    ConcreteScope::Assistant,
                    ConcreteScope::Reasoning,
                    ConcreteScope::Structured,
                ]);
            }
            Scope::Tool => {
                out.extend([ConcreteScope::ToolCall, ConcreteScope::ToolResult]);
            }
            Scope::Title => _ = out.insert(ConcreteScope::Title),
            Scope::User => _ = out.insert(ConcreteScope::User),
            Scope::Assistant => _ = out.insert(ConcreteScope::Assistant),
            Scope::Reasoning => _ = out.insert(ConcreteScope::Reasoning),
            Scope::Structured => _ = out.insert(ConcreteScope::Structured),
            Scope::ToolCall => _ = out.insert(ConcreteScope::ToolCall),
            Scope::ToolResult => _ = out.insert(ConcreteScope::ToolResult),
            Scope::Inquiry => _ = out.insert(ConcreteScope::Inquiry),
        }
    }
    out
}

/// A single output line from a grep search.
struct Hit {
    /// The target conversation ID.
    id: ConversationId,

    /// Where in the conversation this line came from.
    scope: ConcreteScope,

    /// The line text.
    text: String,

    /// If false, this is a "context" line.
    is_match: bool,

    /// Whether a `--` group separator should precede this line.
    group_break: bool,
}

/// Run the match+context pipeline for a single scope source and append hits.
fn collect_scope_hits(
    hits: &mut Vec<Hit>,
    id: ConversationId,
    scope: ConcreteScope,
    lines: &[&str],
    pattern: &str,
    ignore_case: bool,
    context: usize,
) {
    if lines.is_empty() {
        return;
    }

    let match_indices = matching_lines(lines, pattern, ignore_case);
    if match_indices.is_empty() {
        return;
    }

    let ranges = context_ranges(&match_indices, context, lines.len());
    for (range_idx, (start, end)) in ranges.iter().enumerate() {
        for (i, line) in lines.iter().enumerate().skip(*start).take(end - start + 1) {
            hits.push(Hit {
                id,
                scope,
                text: (*line).to_owned(),
                is_match: match_indices.contains(&i),
                group_break: range_idx > 0 && i == *start,
            });
        }
    }
}

fn title_for(ctx: &Ctx, handle: &ConversationHandle) -> Option<String> {
    ctx.workspace
        .metadata(handle)
        .ok()
        .and_then(|m| m.title.clone())
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

/// Which concrete scope an event kind's text belongs to, if any.
fn event_scope(kind: &EventKind) -> Option<ConcreteScope> {
    match kind {
        EventKind::ChatRequest(_) => Some(ConcreteScope::User),
        EventKind::ChatResponse(ChatResponse::Message { .. }) => Some(ConcreteScope::Assistant),
        EventKind::ChatResponse(ChatResponse::Reasoning { .. }) => Some(ConcreteScope::Reasoning),
        EventKind::ChatResponse(ChatResponse::Structured { .. }) => Some(ConcreteScope::Structured),
        EventKind::ToolCallRequest(_) => Some(ConcreteScope::ToolCall),
        EventKind::ToolCallResponse(_) => Some(ConcreteScope::ToolResult),
        EventKind::InquiryRequest(_) => Some(ConcreteScope::Inquiry),
        EventKind::InquiryResponse(_) | EventKind::TurnStart(_) => None,
    }
}

/// Extract all searchable text lines from an event.
///
/// Lines may be borrowed from the event or owned (tool call arguments are
/// serialized on demand).
fn event_lines(kind: &EventKind) -> Vec<Cow<'_, str>> {
    match kind {
        EventKind::ChatRequest(req) => req.content.lines().map(Cow::Borrowed).collect(),
        EventKind::ChatResponse(ChatResponse::Message { message }) => {
            message.lines().map(Cow::Borrowed).collect()
        }
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => {
            reasoning.lines().map(Cow::Borrowed).collect()
        }
        EventKind::ChatResponse(ChatResponse::Structured { data }) => data
            .as_str()
            .iter()
            .flat_map(|text| text.lines())
            .map(Cow::Borrowed)
            .collect(),
        EventKind::ToolCallRequest(req) => {
            let mut out: Vec<Cow<'_, str>> = req.name.lines().map(Cow::Borrowed).collect();
            if !req.arguments.is_empty() {
                // Pretty-print so keys/values land on their own lines; that
                // gives meaningful `--context` behavior and avoids having one
                // giant blob.
                if let Ok(json) = serde_json::to_string_pretty(&req.arguments) {
                    for line in json.lines() {
                        out.push(Cow::Owned(line.to_owned()));
                    }
                }
            }
            out
        }
        EventKind::ToolCallResponse(resp) => resp.content().lines().map(Cow::Borrowed).collect(),
        EventKind::InquiryRequest(req) => req.question.text.lines().map(Cow::Borrowed).collect(),
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
