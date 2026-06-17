//! Text-search primitives over conversations.
//!
//! Both `c grep` (full hit collection with context) and `c use --grep` (boolean
//! filter over conversation IDs) need to walk a conversation's searchable text.
//! The data primitives — what counts as a "scope," how to pull text out of an
//! event, how to read the title — live here so neither command has to depend
//! on the other.

use std::borrow::Cow;

use jp_conversation::{ConversationId, EventKind, event::ChatResponse};
use jp_workspace::ConversationHandle;
use rayon::prelude::*;
use tracing::warn;

use crate::ctx::Ctx;

/// The leaf partitioning of conversation content — the actual surfaces against
/// which text search runs.
/// User-facing meta-scopes (`all`, `chat`, `tool`) are defined by callers and
/// expanded into sets of these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConcreteScope {
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
    pub(crate) const ALL: [Self; 8] = [
        Self::Title,
        Self::User,
        Self::Assistant,
        Self::Reasoning,
        Self::Structured,
        Self::ToolCall,
        Self::ToolResult,
        Self::Inquiry,
    ];

    pub(crate) const fn as_str(self) -> &'static str {
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

impl std::fmt::Display for ConcreteScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which concrete scope an event kind's text belongs to, if any.
pub(crate) fn event_scope(kind: &EventKind) -> Option<ConcreteScope> {
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
pub(crate) fn event_lines(kind: &EventKind) -> Vec<Cow<'_, str>> {
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

/// Read the conversation's title from its metadata.
pub(crate) fn title_for(ctx: &Ctx, handle: &ConversationHandle) -> Option<String> {
    ctx.workspace
        .metadata(handle)
        .ok()
        .and_then(|m| m.title.clone())
}

/// Case-aware substring test.
///
/// When `ignore_case` is true, `needle` is expected to already be lowercased;
/// only the haystack is lowercased per call.
pub(crate) fn contains_substr(haystack: &str, needle: &str, ignore_case: bool) -> bool {
    if ignore_case {
        haystack.to_lowercase().contains(needle)
    } else {
        haystack.contains(needle)
    }
}

/// A compiled match predicate over a single line of text.
///
/// `c grep` builds one of these from the user's pattern and reuses it across
/// every line of every conversation it searches.
pub(crate) enum Matcher {
    /// Substring match.
    /// `needle` is pre-lowercased when `ignore_case` is set.
    Substring { needle: String, ignore_case: bool },

    /// Regular-expression match.
    /// `fancy-regex` supports look-around and backreferences in addition to the
    /// standard syntax.
    Regex(Box<fancy_regex::Regex>),
}

impl Matcher {
    /// Build a substring matcher.
    pub(crate) fn substring(pattern: &str, ignore_case: bool) -> Self {
        let needle = if ignore_case {
            pattern.to_lowercase()
        } else {
            pattern.to_owned()
        };
        Self::Substring {
            needle,
            ignore_case,
        }
    }

    /// Compile a regular-expression matcher.
    pub(crate) fn regex(pattern: &str, ignore_case: bool) -> Result<Self, fancy_regex::Error> {
        let re = fancy_regex::RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()?;
        Ok(Self::Regex(Box::new(re)))
    }

    /// Whether `line` matches.
    pub(crate) fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Substring {
                needle,
                ignore_case,
            } => contains_substr(line, needle, *ignore_case),
            // A regex that errors mid-match (e.g. an exceeded backtrack limit on
            // a fancy pattern) counts as a non-match rather than aborting the
            // whole search.
            Self::Regex(re) => re.is_match(line).unwrap_or(false),
        }
    }
}

/// Filter conversation IDs to those whose title or chat content contains
/// `pattern` as a substring.
///
/// Smart-case: case-insensitive unless `pattern` contains an uppercase
/// character.
/// Searched scopes: title, user, assistant, reasoning, and structured.
/// Runs in parallel via rayon and short-circuits on the first match per
/// conversation.
pub(crate) fn filter_ids(ctx: &Ctx, ids: &[ConversationId], pattern: &str) -> Vec<ConversationId> {
    let ignore_case = !pattern.chars().any(char::is_uppercase);
    let needle = if ignore_case {
        pattern.to_lowercase()
    } else {
        pattern.to_owned()
    };

    ids.par_iter()
        .copied()
        .filter(|id| id_matches(ctx, *id, &needle, ignore_case))
        .collect()
}

/// Whether the conversation's title or chat content contains `needle`.
///
/// `needle` is expected to be pre-lowercased when `ignore_case` is true.
fn id_matches(ctx: &Ctx, id: ConversationId, needle: &str, ignore_case: bool) -> bool {
    let Ok(handle) = ctx.workspace.acquire_conversation(&id) else {
        return false;
    };

    if let Some(title) = title_for(ctx, &handle)
        && contains_substr(&title, needle, ignore_case)
    {
        return true;
    }

    let events = match ctx.workspace.events(&handle) {
        Ok(events) => events,
        Err(error) => {
            warn!(%id, %error, "Failed to load conversation events");
            return false;
        }
    };

    for event in events.iter() {
        let Some(scope) = event_scope(&event.event.kind) else {
            continue;
        };
        if !matches!(
            scope,
            ConcreteScope::User
                | ConcreteScope::Assistant
                | ConcreteScope::Reasoning
                | ConcreteScope::Structured
        ) {
            continue;
        }
        for line in event_lines(&event.event.kind) {
            if contains_substr(&line, needle, ignore_case) {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
