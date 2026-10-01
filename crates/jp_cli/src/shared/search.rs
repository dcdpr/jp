//! Text-search primitives over conversations.
//!
//! Both `c grep` (full hit collection with context) and `c use --grep` (boolean
//! filter over conversation IDs) need to walk a conversation's searchable text.
//! The data primitives — what counts as a "scope," how to pull text out of an
//! event, how to read the title — live here so neither command has to depend
//! on the other.

use std::{
    borrow::Cow,
    ops::Range,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use jp_conversation::{ConversationId, EventKind, Title, event::ChatResponse};
use jp_workspace::ConversationHandle;
use rayon::prelude::*;
use regex::RegexBuilder;
use tracing::warn;

use crate::{ctx::Ctx, format::label_text};

/// The leaf partitioning of conversation content — the actual surfaces against
/// which text search runs.
/// User-facing meta-scopes (`all`, `chat`, `tool`) are defined by callers and
/// expanded into sets of these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ConcreteScope {
    Title,
    Label,
    User,
    Assistant,
    Reasoning,
    Structured,
    ToolCall,
    ToolResult,
    Inquiry,
}

impl ConcreteScope {
    pub(crate) const ALL: [Self; 9] = [
        Self::Title,
        Self::Label,
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
            Self::Label => "label",
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
/// Lines may be borrowed from the event or owned (tool call arguments and
/// structured responses are serialized on demand).
pub(crate) fn event_lines(kind: &EventKind) -> Vec<Cow<'_, str>> {
    match kind {
        EventKind::ChatRequest(req) => req.content.lines().map(Cow::Borrowed).collect(),
        EventKind::ChatResponse(ChatResponse::Message { message }) => {
            message.lines().map(Cow::Borrowed).collect()
        }
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => {
            reasoning.lines().map(Cow::Borrowed).collect()
        }
        EventKind::ChatResponse(ChatResponse::Structured { data }) => match data.as_str() {
            // A response whose JSON failed to parse is kept as a raw string.
            // Searching it verbatim avoids re-quoting and escaping it.
            Some(text) => text.lines().map(Cow::Borrowed).collect(),
            // Anything else was parsed into a `Value` before it was persisted,
            // so its text has to be rebuilt. Pretty-printed for the same reason
            // tool call arguments are.
            None => serde_json::to_string_pretty(data)
                .map(|json| {
                    json.lines()
                        .map(|line| Cow::Owned(line.to_owned()))
                        .collect()
                })
                .unwrap_or_default(),
        },
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
pub(crate) fn title_for(ctx: &Ctx, handle: &ConversationHandle) -> Option<Title> {
    ctx.workspace
        .metadata(handle)
        .ok()
        .and_then(|m| m.title.clone())
}

/// The conversation's labels as searchable lines, one per key-value pair.
///
/// A pair reads as `key=value`, and a bare label as its key alone — the same
/// text `--label` takes — so a pattern can match a key, a value, or the whole
/// pair.
/// Keys are sorted, and a key's values keep the order they were added in.
pub(crate) fn label_lines(ctx: &Ctx, handle: &ConversationHandle) -> Vec<String> {
    let Ok(metadata) = ctx.workspace.metadata(handle) else {
        return vec![];
    };

    metadata
        .labels
        .iter()
        .flat_map(|(key, values)| values.iter().map(|value| label_text(key, value)))
        .collect()
}

/// Whether matching should ignore case.
///
/// An `explicit` preference wins.
/// Without one, smart-case applies: an all-lowercase pattern matches
/// case-insensitively, and any uppercase character in the pattern makes the
/// whole pattern case-sensitive.
fn resolve_ignore_case(pattern: &str, explicit: Option<bool>) -> bool {
    explicit.unwrap_or_else(|| !pattern.chars().any(char::is_uppercase))
}

/// One compiled pattern of a [`Matcher`].
enum Pattern {
    /// A literal pattern.
    ///
    /// Compiled as an escaped regex rather than scanned as a substring: the
    /// engine folds case inside the automaton, so reported offsets stay valid
    /// against the original line even when pattern and text differ in case.
    Literal(Box<regex::Regex>),

    /// A regular expression.
    /// `fancy-regex` supports look-around and backreferences in addition to the
    /// standard syntax.
    Regex(Box<fancy_regex::Regex>),
}

/// A compiled match predicate over a single line of text.
///
/// Holds one or more patterns, and a line matches when any of them does.
/// `c grep` builds one of these from the user's patterns and reuses it across
/// every line of every conversation it searches.
/// Match positions are reported as byte ranges into the line exactly as it was
/// passed in, so a caller can highlight what matched.
/// Which patterns matched is tracked separately, in a [`Coverage`].
///
/// A `fancy-regex` pattern can fail part-way through a search — an exceeded
/// backtrack limit is the common case — which is a different outcome from
/// finding nothing.
/// Such a failure is recorded rather than swallowed; check [`Self::failure`]
/// once the search is done and report it instead of the results.
pub(crate) struct Matcher {
    patterns: Vec<Pattern>,

    /// Whether a match attempt has failed.
    ///
    /// Separate from `message` so the common path is one relaxed atomic load
    /// rather than a mutex acquisition.
    failed: AtomicBool,

    /// The first failure's message, kept for the error shown to the user.
    message: Mutex<Option<String>>,

    /// Whether a recorded failure short-circuits further match attempts.
    ///
    /// See [`Self::ungated`] for when it must be off.
    gate_on_failure: bool,
}

impl Matcher {
    /// Compile `patterns`, treating each as a regular expression when `regex`
    /// is set and as a literal otherwise.
    ///
    /// `explicit_case` forces case-insensitive (`Some(true)`) or case-sensitive
    /// (`Some(false)`) matching for every pattern.
    /// Without it, smart-case decides for each pattern on its own: an
    /// all-lowercase pattern ignores case, one with an uppercase character
    /// respects it.
    ///
    /// The error is the underlying engine's message, suitable for showing to
    /// the user.
    /// When there are several patterns it is prefixed with the one that failed.
    pub(crate) fn new<S: AsRef<str>>(
        patterns: &[S],
        regex: bool,
        explicit_case: Option<bool>,
    ) -> Result<Self, String> {
        let patterns = patterns
            .iter()
            .map(|pattern| {
                let pattern = pattern.as_ref();
                let ignore_case = resolve_ignore_case(pattern, explicit_case);
                compile(pattern, regex, ignore_case).map_err(|error| {
                    if patterns.len() > 1 {
                        format!("`{pattern}`: {error}")
                    } else {
                        error
                    }
                })
            })
            .collect::<Result<_, _>>()?;

        Ok(Self {
            patterns,
            failed: AtomicBool::new(false),
            message: Mutex::new(None),
            gate_on_failure: true,
        })
    }

    /// Keep recording failures, but stop short-circuiting on them.
    ///
    /// The short-circuit saves work when a failure will discard every result.
    /// A caller whose answer *survives* a failure must not use it: with the
    /// gate on, whether a worker reaches the failing input before the answering
    /// one decides the outcome.
    /// `grep -q` is that caller — one found match is the whole answer — and
    /// there is no work to save there anyway, since the search stops at the
    /// first match.
    #[must_use]
    pub(crate) fn ungated(mut self) -> Self {
        self.gate_on_failure = false;
        self
    }

    /// The first match failure, if any attempt has failed.
    ///
    /// A search that reports a failure has an unknown result: it may have found
    /// fewer matches than the pattern describes, so the failure supersedes
    /// whatever was collected.
    pub(crate) fn failure(&self) -> Option<String> {
        if !self.failed.load(Ordering::Relaxed) {
            return None;
        }

        self.message
            .lock()
            .ok()
            .and_then(|message| message.clone())
            .or_else(|| Some("pattern matching failed".to_owned()))
    }

    /// Record a match failure, keeping the first message seen.
    fn fail(&self, error: &fancy_regex::Error) {
        if let Ok(mut message) = self.message.lock()
            && message.is_none()
        {
            *message = Some(error.to_string());
        }

        self.failed.store(true, Ordering::Relaxed);
    }

    /// Whether any pattern matches `line`.
    ///
    /// A failed attempt reports `false` and is recorded; see [`Self::failure`].
    pub(crate) fn is_match(&self, line: &str) -> bool {
        self.patterns
            .iter()
            .any(|pattern| self.pattern_matches(pattern, line))
    }

    /// Whether any pattern matches `line`, marking each one that does in
    /// `coverage`.
    ///
    /// Every pattern is tried, even after one has matched, so `coverage` learns
    /// about all of them.
    pub(crate) fn record_match(&self, line: &str, coverage: &mut Coverage) -> bool {
        let mut matched = false;
        for (seen, pattern) in coverage.seen.iter_mut().zip(&self.patterns) {
            if self.pattern_matches(pattern, line) {
                *seen = true;
                matched = true;
            }
        }

        matched
    }

    /// Whether `pattern` matches `line`.
    ///
    /// A failed attempt reports `false` and is recorded; see [`Self::failure`].
    fn pattern_matches(&self, pattern: &Pattern, line: &str) -> bool {
        match pattern {
            Pattern::Literal(re) => re.is_match(line),
            Pattern::Regex(re) => {
                // A recorded failure supersedes every result, so further
                // attempts are wasted work — and each one can burn the full
                // backtrack limit again. Workers mid-attempt still finish that
                // attempt; this stops the next one.
                if self.gate_on_failure && self.failed.load(Ordering::Relaxed) {
                    return false;
                }

                match re.is_match(line) {
                    Ok(matched) => matched,
                    Err(error) => {
                        self.fail(&error);
                        false
                    }
                }
            }
        }
    }

    /// Byte ranges of every match of every pattern in `line`, in order.
    ///
    /// Spans from different patterns that overlap are merged into one, so no
    /// byte is covered twice.
    /// Zero-width matches are skipped: an empty pattern matches at every
    /// position and there is nothing there to highlight.
    /// A failed attempt ends that pattern's scan, keeping the spans already
    /// found, and is recorded; see [`Self::failure`].
    pub(crate) fn find_spans(&self, line: &str) -> Vec<Range<usize>> {
        let mut spans = Vec::new();
        for pattern in &self.patterns {
            self.pattern_spans(pattern, line, &mut spans);
        }

        spans.retain(|span| !span.is_empty());
        if self.patterns.len() > 1 {
            spans = merge_overlaps(spans);
        }

        spans
    }

    /// Append the byte range of every match of `pattern` in `line` to `spans`.
    fn pattern_spans(&self, pattern: &Pattern, line: &str, spans: &mut Vec<Range<usize>>) {
        match pattern {
            Pattern::Literal(re) => spans.extend(re.find_iter(line).map(|m| m.start()..m.end())),
            Pattern::Regex(re) => {
                // Same early exit as `is_match`: a poisoned matcher stops
                // paying the backtrack limit for results nobody will see.
                if self.gate_on_failure && self.failed.load(Ordering::Relaxed) {
                    return;
                }

                for found in re.find_iter(line) {
                    match found {
                        Ok(m) => spans.push(m.start()..m.end()),
                        Err(error) => {
                            self.fail(&error);
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Which of a [`Matcher`]'s patterns have matched at least one line so far.
///
/// Filled in by [`Matcher::record_match`].
pub(crate) struct Coverage {
    /// One entry per pattern, in the order the matcher was given them.
    seen: Vec<bool>,
}

impl Coverage {
    /// A record of `matcher`'s patterns in which none has matched yet.
    pub(crate) fn new(matcher: &Matcher) -> Self {
        Self {
            seen: vec![false; matcher.patterns.len()],
        }
    }

    /// Whether every pattern has matched at least one line.
    pub(crate) fn is_complete(&self) -> bool {
        self.seen.iter().all(|seen| *seen)
    }
}

/// Compile one pattern, as a regular expression when `regex` is set and as a
/// literal otherwise.
fn compile(pattern: &str, regex: bool, ignore_case: bool) -> Result<Pattern, String> {
    if regex {
        return fancy_regex::RegexBuilder::new(pattern)
            .case_insensitive(ignore_case)
            .build()
            .map(|re| Pattern::Regex(Box::new(re)))
            .map_err(|e| e.to_string());
    }

    RegexBuilder::new(&regex::escape(pattern))
        .case_insensitive(ignore_case)
        .build()
        .map(|re| Pattern::Literal(Box::new(re)))
        .map_err(|e| e.to_string())
}

/// Sort `spans` by position and merge the ones that overlap.
///
/// Spans that only touch stay separate: each is a match of its own.
fn merge_overlaps(mut spans: Vec<Range<usize>>) -> Vec<Range<usize>> {
    spans.sort_by_key(|span| (span.start, span.end));

    let mut merged: Vec<Range<usize>> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(last) = merged.last_mut()
            && span.start < last.end
        {
            last.end = last.end.max(span.end);
            continue;
        }

        merged.push(span);
    }

    merged
}

/// Filter conversation IDs to those containing `pattern` as a literal.
///
/// Smart-case: case-insensitive unless `pattern` contains an uppercase
/// character.
/// Every searchable scope is read: title, labels, chat text, reasoning,
/// structured output, tool call names and arguments, tool results, and inquiry
/// questions.
/// That is the same set `conversation grep` searches by default.
/// Runs in parallel via rayon and short-circuits on the first match per
/// conversation.
///
/// Returns the compiler's message when `pattern` cannot be compiled, which for
/// a literal means only that it exceeded the engine's size limit.
pub(crate) fn filter_ids(
    ctx: &Ctx,
    ids: &[ConversationId],
    pattern: &str,
) -> Result<Vec<ConversationId>, String> {
    let matcher = Matcher::new(&[pattern], false, None)?;

    Ok(ids
        .par_iter()
        .copied()
        .filter(|id| id_matches(ctx, *id, &matcher))
        .collect())
}

/// Whether any of the conversation's searchable text matches.
fn id_matches(ctx: &Ctx, id: ConversationId, matcher: &Matcher) -> bool {
    let Ok(handle) = ctx.workspace.acquire_conversation(&id) else {
        return false;
    };

    if let Some(title) = title_for(ctx, &handle)
        && matcher.is_match(title.raw())
    {
        return true;
    }

    if label_lines(ctx, &handle)
        .iter()
        .any(|line| matcher.is_match(line))
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
        for line in event_lines(&event.event.kind) {
            if matcher.is_match(&line) {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
