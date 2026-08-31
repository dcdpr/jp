//! Compaction projection logic.
//!
//! Transforms a conversation event stream by applying compaction overlays.
//! The original events are consumed and a new projected event list is produced.
//!
//! See [`apply`] for the entry point.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::InternalEvent;
use crate::{
    ByteSize, Compaction, PolicySpec, ReasoningPolicy, ToolCallPolicy,
    event::{ChatRequest, ChatResponse, ConversationEvent, TurnStart},
};

/// Which raw conversation turn(s) a projected turn stands for.
///
/// Turn numbering from [`IterTurns`] is positional, which matches the raw
/// stream but shifts once projection collapses a summarized range into one
/// synthetic turn.
/// [`apply`] returns one `TurnOrigin` per resulting turn so callers can display
/// the original (pre-projection) turn numbers.
///
/// [`IterTurns`]: super::IterTurns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOrigin {
    /// A turn carried through projection unchanged, at this 0-based raw turn
    /// index.
    Kept(usize),
    /// A synthetic summary turn replacing the raw turns `from..=to` (0-based,
    /// inclusive).
    Summary {
        /// First raw turn the summary replaces.
        from: usize,
        /// Last raw turn the summary replaces.
        to: usize,
    },
}

impl TurnOrigin {
    /// Whether this projected turn represents any raw turn in `from..=to`
    /// (0-based, inclusive).
    ///
    /// Lets a selection resolved against raw turn numbers pick the projected
    /// turns to render, including a summary turn that stands in for part of the
    /// range.
    #[must_use]
    pub const fn overlaps(&self, from: usize, to: usize) -> bool {
        match *self {
            Self::Kept(index) => from <= index && index <= to,
            Self::Summary { from: f, to: t } => f <= to && from <= t,
        }
    }
}

/// Resolved compaction policies for a single turn.
struct TurnPolicy {
    /// Summary covering this turn.
    /// Takes precedence over per-type policies.
    summary: Option<ResolvedSummary>,
    /// Reasoning policy, with any size threshold that qualifies it.
    /// Ignored when `summary` is set.
    reasoning: Option<PolicySpec<ReasoningPolicy>>,
    /// Tool call policy, with any size threshold that qualifies it.
    /// Ignored when `summary` is set.
    tool_calls: Option<PolicySpec<ToolCallPolicy>>,
}

/// A summary that won the latest-timestamp contest for a set of turns.
///
/// Equality carries the originating compaction's identity (its turn range and
/// timestamp), not just the text.
/// `inject_at_turn` treats a contiguous run of turns with equal
/// `ResolvedSummary` as one injected summary, so every turn a single summary
/// covers must compare equal (same source), while two distinct adjacent summary
/// compactions that happen to produce identical text must compare unequal and
/// stay separate synthetic turns.
#[derive(PartialEq, Eq)]
struct ResolvedSummary {
    /// The summary text to inject.
    text: String,
    /// First turn of the originating compaction's range.
    from_turn: usize,
    /// Last turn of the originating compaction's range.
    to_turn: usize,
    /// Timestamp of the originating compaction.
    timestamp: DateTime<Utc>,
}

/// Apply compaction projection to the event list in place.
///
/// Reads all [`Compaction`] events, resolves per-turn policies using
/// latest-timestamp-wins semantics, then walks the events to apply:
///
/// - **Summary**: replaces all events in the covered range with a single
///   synthetic `ChatRequest`/`ChatResponse::Message` pair.
/// - **Reasoning strip**: removes `ChatResponse::Reasoning` events.
/// - **Tool call strip**: blanks request arguments and/or replaces response
///   content with a status line.
/// - **Tool call omit**: removes tool call request/response pairs.
///
/// Returns one [`TurnOrigin`] per resulting turn, in turn order, mapping each
/// projected turn back to the raw turn number(s) it represents.
///
/// [`Compaction`]: crate::Compaction
pub(super) fn apply(events: &mut Vec<InternalEvent>) -> Vec<TurnOrigin> {
    let compactions: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            InternalEvent::Compaction(c) => Some(c.clone()),
            _ => None,
        })
        .collect();

    if compactions.is_empty() {
        // Nothing collapses, so every turn maps to itself.
        let event_origins: Vec<TurnOrigin> = assign_turn_indices(events)
            .into_iter()
            .map(TurnOrigin::Kept)
            .collect();
        return collect_turn_origins(events, &event_origins);
    }

    let turn_indices = assign_turn_indices(events);
    let max_turn = turn_indices.iter().copied().max().unwrap_or(0);
    let policies = resolve_policies(max_turn, &compactions);
    let tool_calls = build_tool_calls(events, &turn_indices);

    // Inject a summary once per contiguous run of turns that resolve to the
    // same winning summary. Injecting only at the originating `from_turn` drops
    // the tail of a summary that a newer, fully-contained summary splits in two
    // (e.g. A covers turns 0..=9, a newer B covers 3..=5: turns 6..=9 still
    // belong to A and must be re-injected after B).
    let inject_at_turn: HashSet<usize> = (0..policies.len())
        .filter(|&t| {
            let Some(summary) = policies[t].summary.as_ref() else {
                return false;
            };
            t == 0 || policies[t - 1].summary.as_ref() != Some(summary)
        })
        .collect();

    let mut projected = Vec::with_capacity(events.len());
    // Raw origin of each projected event, kept in lockstep with `projected` so
    // turn numbering can recover the pre-projection turn numbers.
    let mut event_origins: Vec<TurnOrigin> = Vec::with_capacity(events.len());
    let mut summaries_injected: HashSet<usize> = HashSet::new();

    for (i, event) in std::mem::take(events).into_iter().enumerate() {
        let turn = turn_indices[i];

        match event {
            // Config deltas carry global state; unknown (forward-compat) events
            // are opaque. Both pass through projection verbatim — the iterators
            // skip unknown events, so they stay invisible to providers.
            InternalEvent::ConfigDelta(_) | InternalEvent::Unknown(_) => {
                projected.push(event);
                event_origins.push(TurnOrigin::Kept(turn));
            }
            // Compaction events are consumed by projection — they've been
            // applied and should not survive into the projected stream.
            InternalEvent::Compaction(_) => {}
            InternalEvent::Event(conv_event) => {
                let Some(policy) = policies.get(turn) else {
                    projected.push(InternalEvent::Event(conv_event));
                    event_origins.push(TurnOrigin::Kept(turn));
                    continue;
                };

                // Summary takes precedence over all per-type policies.
                if let Some(summary) = &policy.summary {
                    if inject_at_turn.contains(&turn) && summaries_injected.insert(turn) {
                        // The injected summary stands in for the contiguous run
                        // of raw turns that resolve to this same summary — that
                        // is what visually collapses into one turn. Display the
                        // run, not the compaction's declared range, which a
                        // newer fully-contained summary can split in two.
                        let mut run_to = turn;
                        while run_to + 1 < policies.len()
                            && policies[run_to + 1].summary.as_ref() == Some(summary)
                        {
                            run_to += 1;
                        }
                        inject_summary(
                            &mut projected,
                            &mut event_origins,
                            &summary.text,
                            conv_event.timestamp,
                            turn,
                            run_to,
                        );
                    }
                    // Drop the original event — it's covered by the summary.
                    continue;
                }

                let Some(event) = apply_mechanical(*conv_event, policy, tool_calls.get(&i)) else {
                    continue;
                };

                projected.push(InternalEvent::Event(Box::new(event)));
                event_origins.push(TurnOrigin::Kept(turn));
            }
        }
    }

    *events = projected;
    collect_turn_origins(events, &event_origins)
}

/// An item a compaction's mechanical policies reach.
///
/// Reported so a preview can say what a size threshold actually selected.
/// A turn range predicts what it covers; a threshold does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffectedItem {
    /// 0-based raw turn the item sits in.
    pub turn: usize,
    /// What the item is: a tool name qualified by the half being reached
    /// (`fs_read_file (response)`), or `reasoning`.
    pub name: String,
    /// Byte size of the content the policy would remove.
    pub size: ByteSize,
}

/// List the items `compaction`'s mechanical policies reach, in stream order.
///
/// Only the reasoning and tool-call policies select items.
/// A summary replaces every event in its range rather than picking from it, so
/// a compaction carrying one reports nothing even when its mechanical policies
/// are narrowed: projection ignores them.
pub(super) fn affected_items(
    events: &[InternalEvent],
    compaction: &Compaction,
) -> Vec<AffectedItem> {
    if compaction.summary.is_some() {
        return Vec::new();
    }

    let turn_indices = assign_turn_indices(events);
    let tool_calls = build_tool_calls(events, &turn_indices);
    let mut items = Vec::new();

    for (index, entry) in events.iter().enumerate() {
        let turn = turn_indices[index];
        if turn < compaction.from_turn || turn > compaction.to_turn {
            continue;
        }

        let Some(event) = entry.as_event() else {
            continue;
        };

        if let Some(spec) = &compaction.reasoning
            && matches!(spec.policy, ReasoningPolicy::Strip)
            && let Some(response) = event.as_chat_response()
            && response.is_reasoning()
        {
            let size = reasoning_size(response);
            if spec.covers(size) {
                items.push(AffectedItem {
                    turn,
                    name: "reasoning".to_owned(),
                    size: ByteSize::from_bytes(size),
                });
            }
        }

        let Some(spec) = &compaction.tool_calls else {
            continue;
        };
        let Some(info) = tool_calls.get(&index) else {
            continue;
        };

        match &spec.policy {
            // Report the pair once, from its request, since both halves go
            // together.
            ToolCallPolicy::Omit => {
                if event.is_tool_call_request() && spec.covers(info.pair) {
                    items.push(AffectedItem {
                        turn,
                        name: info.name.clone(),
                        size: ByteSize::from_bytes(info.pair),
                    });
                }
            }
            ToolCallPolicy::Strip { request, response } => {
                if *request && event.is_tool_call_request() && spec.covers(info.own) {
                    items.push(AffectedItem {
                        turn,
                        name: format!("{} (request)", info.name),
                        size: ByteSize::from_bytes(info.own),
                    });
                }
                if *response && event.is_tool_call_response() && spec.covers(info.own) {
                    items.push(AffectedItem {
                        turn,
                        name: format!("{} (response)", info.name),
                        size: ByteSize::from_bytes(info.own),
                    });
                }
            }
        }
    }

    items
}

/// Apply a turn's mechanical policies (reasoning and tool calls) to one event.
///
/// Returns `None` when the policies drop the event from the projected view.
/// A policy whose spec carries an `over` threshold reaches only the items
/// larger than it; without one, every item in range is reached.
fn apply_mechanical(
    mut event: ConversationEvent,
    policy: &TurnPolicy,
    info: Option<&ToolCallInfo>,
) -> Option<ConversationEvent> {
    if let Some(spec) = &policy.reasoning
        && matches!(spec.policy, ReasoningPolicy::Strip)
        && let Some(response) = event.as_chat_response()
        && response.is_reasoning()
        && spec.covers(reasoning_size(response))
    {
        return None;
    }

    // A `None` tool-call policy means "no opinion", so the event passes through
    // untouched rather than being dropped.
    if let Some(spec) = policy.tool_calls.as_ref() {
        // A non-tool event has no entry, and reports zero, which no threshold
        // covers.
        let own = info.map_or(0, |i| i.own);
        let pair = info.map_or(0, |i| i.pair);

        match &spec.policy {
            ToolCallPolicy::Omit => {
                // Removing a pair is not a per-half choice, so the threshold is
                // judged on the two halves combined. Both halves read the same
                // total, so a pair is never half-removed.
                if (event.is_tool_call_request() || event.is_tool_call_response())
                    && spec.covers(pair)
                {
                    return None;
                }
            }
            ToolCallPolicy::Strip { request, response } => {
                // Each half is judged on its own size, so a call with a short
                // request and a huge response loses only the response.
                if *request && event.is_tool_call_request() && spec.covers(own) {
                    strip_tool_request(&mut event);
                }
                if *response && event.is_tool_call_response() && spec.covers(own) {
                    strip_tool_response(&mut event, info.map_or("unknown", |i| i.name.as_str()));
                }
            }
        }
    }

    Some(event)
}

/// Group projected events into turns (matching [`IterTurns`]) and return each
/// turn's [`TurnOrigin`], read from the event that opens the turn.
///
/// `event_origins` must be in lockstep with `events`.
///
/// [`IterTurns`]: super::IterTurns
fn collect_turn_origins(events: &[InternalEvent], event_origins: &[TurnOrigin]) -> Vec<TurnOrigin> {
    let mut origins = Vec::new();
    let mut current_has_event = false;
    for (i, event) in events.iter().enumerate() {
        let Some(conv_event) = event.as_event() else {
            continue;
        };
        // A turn opens at the first event and at every later `TurnStart`,
        // mirroring `IterTurns`' boundary rule exactly.
        if !current_has_event || conv_event.is_turn_start() {
            origins.push(event_origins[i]);
        }
        current_has_event = true;
    }
    origins
}

/// Assign a 0-based turn index to each event position.
///
/// Turn boundaries are marked by [`TurnStart`] events, using the same rule as
/// [`IterTurns`]: a `TurnStart` opens a new turn only when the current turn
/// already holds a conversation event.
/// Any conversation events before the first `TurnStart` therefore form an
/// implicit turn 0, and the first explicit `TurnStart` opens turn 1.
/// This must match `IterTurns` exactly, because compaction ranges are created
/// against `iter_turns()` indices but applied here.
///
/// Non-event entries (`ConfigDelta`, `Compaction`, `Unknown`) are invisible to
/// turn iteration; they inherit the current turn index and do not open a turn.
///
/// [`IterTurns`]: super::IterTurns
/// [`TurnStart`]: crate::event::TurnStart
pub(super) fn assign_turn_indices(events: &[InternalEvent]) -> Vec<usize> {
    let mut indices = Vec::with_capacity(events.len());
    let mut turn: usize = 0;
    // Whether the current turn already contains a conversation event. A
    // `TurnStart` only opens a new turn when this is set, mirroring
    // `IterTurns`' "flush when `current` is non-empty" boundary.
    let mut current_has_event = false;

    for event in events {
        match event {
            InternalEvent::Event(ev) => {
                if ev.is_turn_start() && current_has_event {
                    turn += 1;
                }
                indices.push(turn);
                current_has_event = true;
            }
            InternalEvent::ConfigDelta(_)
            | InternalEvent::Compaction(_)
            | InternalEvent::Unknown(_) => {
                indices.push(turn);
            }
        }
    }

    indices
}

/// Resolve the winning compaction policy for each turn.
///
/// For each turn, the compaction with the latest timestamp wins per policy
/// type.
/// Summary, reasoning, and `tool_calls` are resolved independently.
///
/// `compactions` is in stream (stored) order.
/// Ties are broken by that order via `>=`, so a later compaction overrides an
/// earlier one even when both share a timestamp — several compactions
/// generated in one command all call `Compaction::new()` and can land on the
/// same clock reading.
fn resolve_policies(max_turn: usize, compactions: &[crate::Compaction]) -> Vec<TurnPolicy> {
    let count = max_turn + 1;

    let mut policies: Vec<TurnPolicy> = (0..count)
        .map(|_| TurnPolicy {
            summary: None,
            reasoning: None,
            tool_calls: None,
        })
        .collect();

    // Track winning timestamps separately to keep TurnPolicy simple.
    let mut summary_ts: Vec<Option<DateTime<Utc>>> = vec![None; count];
    let mut reasoning_ts: Vec<Option<DateTime<Utc>>> = vec![None; count];
    let mut tool_calls_ts: Vec<Option<DateTime<Utc>>> = vec![None; count];

    for c in compactions {
        let to = c.to_turn.min(max_turn);

        for turn in c.from_turn..=to {
            if c.summary.is_some() && summary_ts[turn].is_none_or(|ts| c.timestamp >= ts) {
                summary_ts[turn] = Some(c.timestamp);
                policies[turn].summary = c.summary.as_ref().map(|s| ResolvedSummary {
                    text: s.summary.clone(),
                    from_turn: c.from_turn,
                    to_turn: c.to_turn,
                    timestamp: c.timestamp,
                });
            }

            if c.reasoning.is_some() && reasoning_ts[turn].is_none_or(|ts| c.timestamp >= ts) {
                reasoning_ts[turn] = Some(c.timestamp);
                policies[turn].reasoning.clone_from(&c.reasoning);
            }

            if c.tool_calls.is_some() && tool_calls_ts[turn].is_none_or(|ts| c.timestamp >= ts) {
                tool_calls_ts[turn] = Some(c.timestamp);
                policies[turn].tool_calls.clone_from(&c.tool_calls);
            }
        }
    }

    policies
}

/// Inject a synthetic `ChatRequest`/`ChatResponse` pair for a summary.
///
/// A leading `TurnStart` keeps the synthetic pair as its own turn so that
/// `iter_turns()` (and `print --compacted --turn/--last`) treats the summary as
/// a distinct turn rather than folding it into the preceding one.
/// The `TurnStart` is not provider-visible, so it is filtered out before the
/// LLM request is built.
///
/// `from`/`to` are the raw turn range this summary replaces; every injected
/// event records it as its [`TurnOrigin`] so the run stays in lockstep with
/// `events`.
fn inject_summary(
    events: &mut Vec<InternalEvent>,
    origins: &mut Vec<TurnOrigin>,
    summary: &str,
    timestamp: DateTime<Utc>,
    from: usize,
    to: usize,
) {
    let origin = TurnOrigin::Summary { from, to };
    events.push(InternalEvent::Event(Box::new(ConversationEvent::new(
        TurnStart, timestamp,
    ))));
    origins.push(origin);
    events.push(InternalEvent::Event(Box::new(ConversationEvent::new(
        ChatRequest::from("[Summary of previous conversation]"),
        timestamp,
    ))));
    origins.push(origin);
    events.push(InternalEvent::Event(Box::new(ConversationEvent::new(
        ChatResponse::message(summary),
        timestamp,
    ))));
    origins.push(origin);
}

/// Blank a tool call request's arguments.
///
/// Arguments are the dominant token sink (file contents, patches, prompts) and
/// aren't needed once a turn is compacted — the tool name, call ID, and (when
/// kept) the response carry the meaning.
/// Emptied to `{}` rather than a placeholder so there is nothing for the model
/// to echo into a live call.
fn strip_tool_request(event: &mut ConversationEvent) {
    if let Some(req) = event.as_tool_call_request_mut() {
        req.arguments = Map::new();
    }
}

/// Replace a tool call response's content with a compact status line.
///
/// `name` is the tool named by the paired request, or `unknown` when no request
/// for it survives in the stream.
fn strip_tool_response(event: &mut ConversationEvent, name: &str) {
    if let Some(resp) = event.as_tool_call_response_mut() {
        let status = if resp.result.is_ok() {
            "success"
        } else {
            "error"
        };
        let line = format!("[compacted] {name}: {status}");
        resp.result = if resp.result.is_ok() {
            Ok(line)
        } else {
            Err(line)
        };
    }
}

/// What the tool call policies need to know about the tool call event at one
/// stream position.
///
/// The name feeds a stripped response's status line; the sizes feed a spec's
/// `over` threshold.
#[derive(Default)]
struct ToolCallInfo {
    /// Name of the tool, taken from the request half of the pair.
    name: String,
    /// Byte size of this event's own half.
    own: u64,
    /// Combined byte size of both halves of the pair.
    ///
    /// Equal to `own` for a half whose partner is missing from the stream.
    pair: u64,
}

/// Map each tool call event's stream position to its name and sizes.
///
/// Keyed by position rather than by call ID because a call ID is not unique: a
/// provider may reuse one synthetic ID across streaming cycles, which
/// [`TurnMut::build`] explicitly permits.
/// Keying by ID would give every occurrence the last one's sizes, so a
/// threshold would reach the wrong halves.
///
/// Pairing mirrors `TurnMut::build`'s count-based rule: a response binds to the
/// oldest request in its turn that carries the same ID and has no response yet.
///
/// [`TurnMut::build`]: super::TurnMut::build
fn build_tool_calls(
    events: &[InternalEvent],
    turn_indices: &[usize],
) -> HashMap<usize, ToolCallInfo> {
    let mut calls: HashMap<usize, ToolCallInfo> = HashMap::new();
    // Positions of requests still awaiting a response, oldest first, per
    // (turn, call ID). Scoped by turn because request-response pairing is
    // turn-local, so an orphaned request cannot capture a later turn's response.
    let mut pending: HashMap<(usize, &str), VecDeque<usize>> = HashMap::new();

    for (index, entry) in events.iter().enumerate() {
        let Some(event) = entry.as_event() else {
            continue;
        };
        let turn = turn_indices[index];

        if let Some(req) = event.as_tool_call_request() {
            let own = arguments_size(&req.arguments);
            calls.insert(index, ToolCallInfo {
                name: req.name.clone(),
                own,
                // Stands alone until its response is found.
                pair: own,
            });
            pending
                .entry((turn, req.id.as_str()))
                .or_default()
                .push_back(index);
        } else if let Some(resp) = event.as_tool_call_response() {
            let own = byte_count(resp.content().len());
            let request = pending
                .get_mut(&(turn, resp.id.as_str()))
                .and_then(VecDeque::pop_front);

            // Both halves must read the same `pair` total, so a threshold on
            // `Omit` either removes a pair or leaves it whole.
            let (name, pair) = match request.and_then(|pos| calls.get_mut(&pos)) {
                Some(request) => {
                    let pair = request.own.saturating_add(own);
                    request.pair = pair;
                    (request.name.clone(), pair)
                }
                None => ("unknown".to_owned(), own),
            };

            calls.insert(index, ToolCallInfo { name, own, pair });
        }
    }

    calls
}

/// Byte size of a tool call request's arguments as the provider receives them.
///
/// Measured on the serialized JSON rather than the stored bytes: arguments are
/// base64-encoded at rest, so the on-disk size is not what reaches the model.
fn arguments_size(arguments: &Map<String, Value>) -> u64 {
    serde_json::to_string(arguments).map_or(0, |json| byte_count(json.len()))
}

/// Byte size of a chat response's reasoning content.
///
/// Any other response kind reports zero.
fn reasoning_size(response: &ChatResponse) -> u64 {
    match response {
        ChatResponse::Reasoning { reasoning } => byte_count(reasoning.len()),
        _ => 0,
    }
}

/// Narrow an in-memory length to the width the size thresholds compare against.
fn byte_count(len: usize) -> u64 {
    u64::try_from(len).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
