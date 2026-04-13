//! Rendering pipeline: raw JSON events to HTML-ready types.
//!
//! Works directly with `serde_json::Value` events received from the JP host
//! protocol, without depending on `jp_conversation` types. The host decodes
//! base64-encoded storage fields before sending, so values arrive as plain
//! text.

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

/// Render raw JSON events into [`RenderedEvent`]s for the detail view.
///
/// Events come from the host's `read_events` response with base64 fields
/// already decoded to plain text.
pub(crate) fn render_events(events: &[Value]) -> Vec<RenderedEvent> {
    let mut out = Vec::new();
    let mut is_first_turn = true;

    for event in events {
        let Some(event_type) = event.get("type").and_then(Value::as_str) else {
            continue;
        };

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

            "chat_response" => render_chat_response(event, &mut out),

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

    out
}

/// Handle the untagged `ChatResponse` variants by checking which key is
/// present.
fn render_chat_response(event: &Value, out: &mut Vec<RenderedEvent>) {
    if let Some(msg) = event.get("message").and_then(Value::as_str) {
        out.push(RenderedEvent::AssistantMessage {
            html: markdown_to_html(msg),
        });
    } else if let Some(reasoning) = event.get("reasoning").and_then(Value::as_str) {
        out.push(RenderedEvent::Reasoning {
            html: markdown_to_html(reasoning),
        });
    } else if let Some(data) = event.get("data") {
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

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_owned()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}\n\n... (truncated)")
    }
}

/// Convert markdown to HTML using comrak.
pub(crate) fn markdown_to_html(md: &str) -> String {
    let mut options = comrak::Options::default();
    options.render.r#unsafe = true;
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;

    comrak::markdown_to_html(md, &options)
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
