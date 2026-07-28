//! Fitting a conversation into a model's context window.
//!
//! [`truncate_to_fit`] is the entry point: it applies compaction projection,
//! then drops the oldest events from a stream until a char-based estimate of
//! its size fits the budget derived from the model's context window.
//! Callers measure the fixed overhead sharing that window (system prompt,
//! instruction sections, attachments, tool definitions) with
//! [`estimate_overhead_chars`] and pass it in.
//!
//! Estimation is deliberately crude: a fixed chars-per-token ratio with a
//! safety margin absorbing the error.
//! Everything here is pure — no provider calls, no tokenizer.

use jp_attachment::Attachment;
use jp_config::assistant::sections::SectionConfig;
use jp_conversation::{ConversationEvent, ConversationStream, EventKind, event::ChatResponse};
use tracing::info;

use crate::tool::ToolDefinition;

/// Estimated chars-per-token ratio used for estimation.
pub const CHARS_PER_TOKEN: usize = 3;

/// Safety margin for tokenization imprecision (the chars-per-token ratio varies
/// by content type) and provider framing overhead (JSON wrapping, role tags,
/// structured output injection, etc.).
///
/// System prompt, sections, attachments, and tool definitions are measured
/// explicitly via [`estimate_overhead_chars`], so this factor only needs to
/// cover the remaining approximation error.
const OVERHEAD_FACTOR: usize = 90; // percent

/// When truncation is needed, target this fraction of the context window.
/// Leaves headroom so a later request against the same window doesn't
/// re-truncate at a different cutoff (which would bust the prompt cache).
const TARGET_FACTOR: usize = 80; // percent

/// Char-based estimate of the fixed overhead that shares a model's context
/// window with conversation events.
///
/// Binary attachments consume tokens too (base64 framing, image tiles) but
/// can't be measured in chars; the module's safety margin absorbs them.
#[must_use]
pub fn estimate_overhead_chars(
    system_prompt: Option<&str>,
    sections: &[SectionConfig],
    attachments: &[Attachment],
    tools: &[ToolDefinition],
) -> usize {
    let mut chars = 0;

    if let Some(prompt) = system_prompt {
        chars += prompt.len();
    }

    for section in sections {
        chars += section.render().len();
    }

    for attachment in attachments {
        if let Some(text) = attachment.as_text() {
            chars += text.len();
        }
    }

    for tool in tools {
        chars += tool.name.len();
        if let Some(desc) = tool.docs.schema_description() {
            chars += desc.len();
        }
        // Parameter schemas are serialized as JSON by providers.
        chars += serde_json::to_string(&tool.to_parameters_schema()).map_or(0, |s| s.len());
    }

    chars
}

/// Drop the oldest events until the stream fits the model's context window.
///
/// `overhead_chars` is the measured size of everything else sharing the window
/// (see [`estimate_overhead_chars`]) and is subtracted from the budget.
/// Returns the number of events dropped, which is zero when the stream already
/// fits.
///
/// The stream is left projected: compaction overlays are resolved into the
/// events they imply (see [`ConversationStream::apply_projection`]), which is
/// what the provider receives either way.
///
/// Events are dropped from the start, which keeps the cutoff stable across
/// repeated calls on a growing stream: streams are append-only, so the same K
/// oldest events are dropped no matter how many new events arrive at the end,
/// and a provider's prompt cache prefix survives.
///
/// Structural invariants are restored afterwards via
/// [`ConversationStream::sanitize`] (orphaned tool calls, leading non-user
/// events).
/// If truncation removes every chat request, every remaining conversation event
/// is dropped too: what is left cannot form a valid provider message sequence,
/// since providers require the first message to come from the user.
/// Callers append their own request after fitting, so an emptied stream is
/// still valid input.
/// Config deltas survive that emptying — they are global, provider-invisible,
/// and callers read the stream's effective config to build the request.
pub fn truncate_to_fit(
    events: &mut ConversationStream,
    context_window: u32,
    overhead_chars: usize,
) -> usize {
    // Measure the events the provider will actually receive. A stored summary
    // can stand in for a long range, so the raw stream both over-estimates the
    // request and is the wrong thing to cut: dropping a prefix invalidates every
    // compaction overlay it touches (see `ConversationStream::retain`), which
    // would hand the provider the raw tail a summary was covering.
    events.apply_projection();

    let budget = budget_chars(context_window, overhead_chars);
    let total_chars = estimate_chars(events);

    if total_chars <= budget {
        return 0;
    }

    let target = target_chars(context_window, overhead_chars);

    // Round must_drop up to the nearest 10% of target so that small
    // additions at the end of the stream don't shift the cutoff point. This
    // keeps the prefix stable across repeated calls against the same window,
    // preserving prompt cache hits on the conversation messages.
    let granularity = target / 10;
    let raw_drop = total_chars.saturating_sub(target);
    let must_drop = match granularity {
        0 => raw_drop,
        g => raw_drop.div_ceil(g) * g,
    };

    // Walk from the start (oldest), accumulating chars to drop.
    let char_counts: Vec<usize> = events
        .iter()
        .map(|e| estimate_event_chars(e.event))
        .collect();

    let mut dropped_chars = 0;
    let mut dropped_events = 0;

    for count in &char_counts {
        if dropped_chars >= must_drop {
            break;
        }
        dropped_chars += count;
        dropped_events += 1;
    }

    let mut idx = 0;
    events.retain(|_| {
        let keep = idx >= dropped_events;
        idx += 1;
        keep
    });

    events.sanitize();

    if !events.has_chat_request() {
        // Drop the conversation events, keep the configuration. `retain`
        // preserves global entries, so the config deltas the request is built
        // from outlive the emptying; `clear` would take them with it.
        events.retain(|_| false);
    }

    info!(
        context_window,
        dropped_events, "Truncated conversation to fit the model's context window.",
    );

    dropped_events
}

/// Char-based estimate of the conversation events in a stream.
///
/// Counts provider-visible payloads only; turn markers, config deltas and
/// compaction overlays contribute nothing.
/// The stream is measured as given: to size what a provider would receive,
/// project it first (see [`ConversationStream::apply_projection`]).
#[must_use]
pub fn estimate_chars(events: &ConversationStream) -> usize {
    events.iter().map(|e| estimate_event_chars(e.event)).sum()
}

/// Char-based estimate of a single event's contribution to the window.
///
/// Events with no provider-visible payload count as zero.
fn estimate_event_chars(event: &ConversationEvent) -> usize {
    match &event.kind {
        EventKind::ChatRequest(r) => r.content.len(),
        EventKind::ChatResponse(ChatResponse::Message { message }) => message.len(),
        EventKind::ChatResponse(ChatResponse::Reasoning { reasoning }) => reasoning.len(),
        EventKind::ChatResponse(ChatResponse::Structured { data }) => data.to_string().len(),
        EventKind::ToolCallRequest(r) => {
            r.name.len() + serde_json::to_string(&r.arguments).map_or(0, |s| s.len())
        }
        EventKind::ToolCallResponse(r) => {
            r.result.as_ref().map_or(0, String::len)
                + r.result.as_ref().err().map_or(0, String::len)
        }
        _ => 0,
    }
}

/// Chars available to conversation events before truncation kicks in.
///
/// This is the model's window converted to chars, discounted by the safety
/// margin and reduced by the caller's measured overhead.
/// Saturates at zero when the overhead alone fills the window.
#[must_use]
pub fn budget_chars(context_window: u32, overhead_chars: usize) -> usize {
    let total = (context_window as usize) * CHARS_PER_TOKEN * OVERHEAD_FACTOR / 100;
    total.saturating_sub(overhead_chars)
}

/// Chars a truncated stream is shrunk down to, leaving headroom below the
/// budget.
fn target_chars(context_window: u32, overhead_chars: usize) -> usize {
    let total = (context_window as usize) * CHARS_PER_TOKEN * TARGET_FACTOR / 100;
    total.saturating_sub(overhead_chars)
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
