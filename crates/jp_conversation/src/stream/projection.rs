//! Compaction projection logic.
//!
//! Transforms a conversation event stream by applying compaction overlays.
//! The original events are consumed and a new projected event list is produced.
//!
//! See [`apply`] for the entry point.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::InternalEvent;
use crate::{
    ReasoningPolicy, ToolCallPolicy,
    event::{ChatRequest, ChatResponse, ConversationEvent},
};

/// Resolved compaction policies for a single turn.
struct TurnPolicy {
    /// Summary covering this turn. Takes precedence over per-type policies.
    summary: Option<ResolvedSummary>,
    /// Reasoning policy. Ignored when `summary` is set.
    reasoning: Option<ReasoningPolicy>,
    /// Tool call policy. Ignored when `summary` is set.
    tool_calls: Option<ToolCallPolicy>,
}

/// A summary that won the latest-timestamp contest for a set of turns.
struct ResolvedSummary {
    /// The summary text to inject.
    text: String,
    /// The `from_turn` of the originating compaction, used to determine
    /// where the synthetic pair is injected.
    from_turn: usize,
}

/// Apply compaction projection to the event list in place.
///
/// Reads all [`Compaction`] events, resolves per-turn policies using
/// latest-timestamp-wins semantics, then walks the events to apply:
///
/// - **Summary**: replaces all events in the covered range with a single
///   synthetic `ChatRequest`/`ChatResponse::Message` pair.
/// - **Reasoning strip**: removes `ChatResponse::Reasoning` events.
/// - **Tool call strip**: replaces arguments and/or response content with
///   compact summaries.
/// - **Tool call omit**: removes tool call request/response pairs.
///
/// [`Compaction`]: crate::Compaction
pub(super) fn apply(events: &mut Vec<InternalEvent>) {
    let compactions: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            InternalEvent::Compaction(c) => Some(c.clone()),
            _ => None,
        })
        .collect();

    if compactions.is_empty() {
        return;
    }

    let turn_indices = assign_turn_indices(events);
    let max_turn = turn_indices.iter().copied().max().unwrap_or(0);
    let policies = resolve_policies(max_turn, &compactions);
    let tool_names = build_tool_name_map(events);

    let mut projected = Vec::with_capacity(events.len());
    let mut summaries_injected: HashSet<usize> = HashSet::new();

    for (i, event) in std::mem::take(events).into_iter().enumerate() {
        let turn = turn_indices[i];

        match event {
            InternalEvent::ConfigDelta(_) => {
                projected.push(event);
            }
            // Compaction events are consumed by projection — they've been
            // applied and should not survive into the projected stream.
            InternalEvent::Compaction(_) => {}
            InternalEvent::Event(conv_event) => {
                let Some(policy) = policies.get(turn) else {
                    projected.push(InternalEvent::Event(conv_event));
                    continue;
                };

                // Summary takes precedence over all per-type policies.
                if let Some(summary) = &policy.summary {
                    if summary.from_turn == turn && summaries_injected.insert(turn) {
                        inject_summary(&mut projected, &summary.text, conv_event.timestamp);
                    }
                    // Drop the original event — it's covered by the summary.
                    continue;
                }

                let mut event = *conv_event;

                // Reasoning policy.
                if matches!(policy.reasoning, Some(ReasoningPolicy::Strip))
                    && event
                        .as_chat_response()
                        .is_some_and(ChatResponse::is_reasoning)
                {
                    continue;
                }

                // Tool call policy.
                if let Some(tc_policy) = &policy.tool_calls {
                    match tc_policy {
                        ToolCallPolicy::Omit => {
                            if event.is_tool_call_request() || event.is_tool_call_response() {
                                continue;
                            }
                        }
                        ToolCallPolicy::Strip { request, response } => {
                            if *request {
                                strip_tool_request(&mut event);
                            }
                            if *response {
                                strip_tool_response(&mut event, &tool_names);
                            }
                        }
                    }
                }

                projected.push(InternalEvent::Event(Box::new(event)));
            }
        }
    }

    *events = projected;
}

/// Assign a 0-based turn index to each event position.
///
/// Turn boundaries are marked by [`TurnStart`] events. Everything before the
/// first `TurnStart` (or from the first `TurnStart` onward until the next)
/// belongs to turn 0. Non-event entries (`ConfigDelta`, `Compaction`) inherit
/// the current turn index.
///
/// [`TurnStart`]: crate::event::TurnStart
pub(super) fn assign_turn_indices(events: &[InternalEvent]) -> Vec<usize> {
    let mut indices = Vec::with_capacity(events.len());
    let mut turn: usize = 0;
    let mut first_turn_seen = false;

    for event in events {
        let is_turn_start = matches!(event, InternalEvent::Event(ev) if ev.is_turn_start());

        if is_turn_start && first_turn_seen {
            turn += 1;
        }
        if is_turn_start {
            first_turn_seen = true;
        }

        indices.push(turn);
    }

    indices
}

/// Resolve the winning compaction policy for each turn.
///
/// For each turn, the compaction with the latest timestamp wins per policy
/// type. Summary, reasoning, and `tool_calls` are resolved independently.
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
            if c.summary.is_some() && summary_ts[turn].is_none_or(|ts| c.timestamp > ts) {
                summary_ts[turn] = Some(c.timestamp);
                policies[turn].summary = c.summary.as_ref().map(|s| ResolvedSummary {
                    text: s.summary.clone(),
                    from_turn: c.from_turn,
                });
            }

            if c.reasoning.is_some() && reasoning_ts[turn].is_none_or(|ts| c.timestamp > ts) {
                reasoning_ts[turn] = Some(c.timestamp);
                policies[turn].reasoning.clone_from(&c.reasoning);
            }

            if c.tool_calls.is_some() && tool_calls_ts[turn].is_none_or(|ts| c.timestamp > ts) {
                tool_calls_ts[turn] = Some(c.timestamp);
                policies[turn].tool_calls.clone_from(&c.tool_calls);
            }
        }
    }

    policies
}

/// Inject a synthetic `ChatRequest`/`ChatResponse` pair for a summary.
fn inject_summary(events: &mut Vec<InternalEvent>, summary: &str, timestamp: DateTime<Utc>) {
    events.push(InternalEvent::Event(Box::new(ConversationEvent::new(
        ChatRequest::from("[Summary of previous conversation]"),
        timestamp,
    ))));
    events.push(InternalEvent::Event(Box::new(ConversationEvent::new(
        ChatResponse::message(summary),
        timestamp,
    ))));
}

/// Replace a tool call request's arguments with a compacted marker.
fn strip_tool_request(event: &mut ConversationEvent) {
    if let Some(req) = event.as_tool_call_request_mut() {
        req.arguments = Map::from_iter([("[compacted]".to_owned(), Value::Bool(true))]);
    }
}

/// Replace a tool call response's content with a compact status line.
fn strip_tool_response(event: &mut ConversationEvent, tool_names: &HashMap<String, String>) {
    if let Some(resp) = event.as_tool_call_response_mut() {
        let name = tool_names.get(&resp.id).map_or("unknown", String::as_str);
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

/// Build a map from tool call ID → tool name for response stripping.
fn build_tool_name_map(events: &[InternalEvent]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for event in events {
        if let InternalEvent::Event(ev) = event
            && let Some(req) = ev.as_tool_call_request()
        {
            map.insert(req.id.clone(), req.name.clone());
        }
    }
    map
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod tests;
