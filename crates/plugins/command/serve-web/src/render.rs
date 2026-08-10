//! Rendering pipeline: raw JSON events to HTML-ready types.
//!
//! Works directly with `serde_json::Value` events received from the JP host
//! protocol, without depending on `jp_conversation` types.
//! The host decodes base64-encoded storage fields before sending, so values
//! arrive as plain text.

use std::mem;

use serde_json::Value;

/// A pre-rendered event ready for the detail view template.
pub(crate) enum RenderedEvent {
    TurnSeparator,
    UserMessage {
        html: String,
    },
    AssistantMessage {
        html: String,
    },
    Reasoning {
        html: String,
    },
    Structured {
        json: String,
    },
    ToolCall {
        name: String,
        arguments: String,
        result: Option<String>,
    },
}

/// The first event that can still change, or the end if none can.
///
/// A tool call is rendered when it is requested and gains its result later, so
/// its entry is not final the moment it appears.
/// Anything from here on has to be sent again rather than assumed unchanged —
/// without this, a caller that only ever appends keeps the request and never
/// learns the answer.
pub(crate) fn settled_upto(events: &[RenderedEvent]) -> usize {
    events
        .iter()
        .position(|event| matches!(event, RenderedEvent::ToolCall { result: None, .. }))
        .unwrap_or(events.len())
}

/// Whether the conversation is waiting on the assistant.
///
/// True when the last thing in the transcript is the user's message, or a tool
/// call with no result yet.
/// Read from the transcript rather than from any bookkeeping, so it holds for a
/// turn started from another process, and survives this server restarting
/// mid-turn.
///
/// A turn that was interrupted and never resumed looks the same as one still
/// running.
/// Both are "the assistant owes you a reply", which is what the page reports,
/// so the conflation is honest rather than merely convenient.
pub(crate) fn awaiting_response(events: &[RenderedEvent]) -> bool {
    events
        .iter()
        .rev()
        .find(|event| !matches!(event, RenderedEvent::TurnSeparator))
        .is_some_and(|event| match event {
            RenderedEvent::UserMessage { .. } => true,
            RenderedEvent::ToolCall { result, .. } => result.is_none(),
            RenderedEvent::AssistantMessage { .. }
            | RenderedEvent::Reasoning { .. }
            | RenderedEvent::Structured { .. }
            | RenderedEvent::TurnSeparator => false,
        })
}

/// Which kind of text a [`PendingText`] region holds.
#[derive(Clone, Copy, PartialEq)]
enum TextKind {
    Message,
    Reasoning,
}

/// Accumulates consecutive text-bearing chat responses of one kind, so a region
/// split across several events is parsed as markdown once.
///
/// A provider may deliver one region of text as several events: Anthropic
/// interrupts a thinking block with an opaque `redacted_thinking` block and
/// resumes the thinking after it, splitting the text mid-word.
/// Parsing each event on its own would break any markdown construct spanning
/// the split and show a block boundary the reader never saw.
/// Segmentation *within* a region travels in the text as blank lines.
#[derive(Default)]
struct PendingText {
    kind: Option<TextKind>,
    text: String,
}

impl PendingText {
    /// Append `text` to the open region, closing the previous one first when it
    /// held a different kind.
    fn push(&mut self, kind: TextKind, text: &str, out: &mut Vec<RenderedEvent>) {
        if self.kind != Some(kind) {
            self.flush(out);
            self.kind = Some(kind);
        }
        self.text.push_str(text);
    }

    /// Close the open region, emitting it as a single rendered event.
    ///
    /// A region that holds only whitespace is dropped: an event carrying no
    /// text (a redacted thinking payload, for instance) has nothing to show.
    fn flush(&mut self, out: &mut Vec<RenderedEvent>) {
        let Some(kind) = self.kind.take() else {
            return;
        };

        let text = mem::take(&mut self.text);
        if text.trim().is_empty() {
            return;
        }

        let html = markdown_to_html(&text);
        out.push(match kind {
            TextKind::Message => RenderedEvent::AssistantMessage { html },
            TextKind::Reasoning => RenderedEvent::Reasoning { html },
        });
    }
}

/// Render raw JSON events into [`RenderedEvent`]s for the detail view.
///
/// Events come from the host's `read_events` response with base64 fields
/// already decoded to plain text.
pub(crate) fn render_events(events: &[Value]) -> Vec<RenderedEvent> {
    let mut out = Vec::new();
    let mut pending = PendingText::default();
    let mut is_first_turn = true;

    for event in events {
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            continue;
        };

        // Anything that is not more text of the same kind closes the open
        // region, so it renders ahead of whatever follows it.
        if event_type != "chat_response" {
            pending.flush(&mut out);
        }

        match event_type {
            "turn_start" => {
                if !is_first_turn {
                    out.push(RenderedEvent::TurnSeparator);
                }
                is_first_turn = false;
            }

            "chat_request" => {
                if let Some(content) = event.get("content").and_then(Value::as_str) {
                    out.push(RenderedEvent::UserMessage {
                        html: markdown_to_html(content),
                    });
                }
            }

            "chat_response" => render_chat_response(event, &mut pending, &mut out),

            "tool_call_request" => {
                let name = event
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned();

                let arguments = pretty_print_args(event.get("arguments"));
                let id = event.get("id").and_then(Value::as_str).unwrap_or("");
                let result = find_tool_response(events, id);

                out.push(RenderedEvent::ToolCall {
                    name,
                    arguments,
                    result,
                });
            }

            // tool_call_response: folded into the ToolCall above.
            // config_delta, inquiry_*: skipped.
            _ => {}
        }
    }

    pending.flush(&mut out);

    out
}

/// Handle the untagged `ChatResponse` variants by checking which key is
/// present.
///
/// Message and reasoning text joins the open region in `pending`.
/// A structured response is a discrete JSON value, so it closes the region and
/// is emitted on its own.
fn render_chat_response(event: &Value, pending: &mut PendingText, out: &mut Vec<RenderedEvent>) {
    if let Some(msg) = event.get("message").and_then(Value::as_str) {
        pending.push(TextKind::Message, msg, out);
    } else if let Some(reasoning) = event.get("reasoning").and_then(Value::as_str) {
        pending.push(TextKind::Reasoning, reasoning, out);
    } else if let Some(data) = event.get("data") {
        pending.flush(out);
        let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string());
        out.push(RenderedEvent::Structured { json });
    }
}

/// Find the `tool_call_response` matching a given request ID.
fn find_tool_response(events: &[Value], id: &str) -> Option<String> {
    events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("tool_call_response"))
        .find(|e| e.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|e| e.get("content").and_then(Value::as_str))
        .map(|s| truncate(s, 10_000))
}

/// Pretty-print tool call arguments.
fn pretty_print_args(value: Option<&Value>) -> String {
    let Some(val) = value else {
        return String::new();
    };
    serde_json::to_string_pretty(val).unwrap_or_else(|_| val.to_string())
}

/// Truncate to at most `max` bytes, cutting on a character boundary.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n... (truncated)", &s[..end])
}

/// Convert markdown to HTML using comrak.
pub(crate) fn markdown_to_html(md: &str) -> String {
    // Raw HTML in the markdown is escaped, not passed through: conversation
    // content is untrusted (tool output, fetched pages, file contents) and the
    // result is injected into the page verbatim, so raw HTML would be a stored
    // XSS vector.
    let mut options = comrak::Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;

    comrak::markdown_to_html(md, &options)
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
