pub(crate) mod conversation;
pub(crate) mod datetime;

use jp_config::types::color::Color;
use jp_conversation::{Compaction, ToolCallPolicy};
use jp_term::table::DetailItem;
use serde_json::json;
use url::Url;

/// Build a list item for an attachment URL.
///
/// The terminal text reads as `scheme (description): url` when the attachment
/// carries a `description` query parameter, and as the bare URL otherwise.
/// The JSON form is always an object with `scheme`, `description` (null when
/// absent), and the canonical `url`.
pub(crate) fn attachment_detail_item(url: &Url) -> DetailItem {
    let scheme = url.scheme();
    let description = url
        .query_pairs()
        .find(|(key, _)| key == "description")
        .map(|(_, value)| value.into_owned());
    let url_str = url.to_string();

    let text = match &description {
        Some(description) => format!("{scheme} ({description}): {url_str}"),
        None => url_str.clone(),
    };

    DetailItem::new(
        text,
        json!({
            "scheme": scheme,
            "description": description,
            "url": url_str,
        }),
    )
}

/// Build a list item for a persisted compaction.
///
/// The terminal text reads as `turns X..Y (N total, POLICY)`, where `POLICY` is
/// `summary` when the range was replaced by a generated summary, or a
/// description of the applied reasoning/tool-call policies (e.g. `reasoning +
/// tools`) otherwise.
/// The JSON form is always an object with `from_turn`, `to_turn` (1-based,
/// inclusive), `reasoning`, `tool_calls`, and `summary` (the full generated
/// text, or `null`).
///
/// `tool_calls` mirrors [`ToolCallPolicy`]'s own serialized shape (e.g.
/// `{"policy": "strip", "request": true, "response": true}`) rather than the
/// `--tools` flag vocabulary, since a policy can carry `request`/`response`
/// combinations the flag can't express.
pub(crate) fn compaction_detail_item(compaction: &Compaction) -> DetailItem {
    let from = compaction.from_turn + 1;
    let to = compaction.to_turn + 1;
    let count = compaction.to_turn - compaction.from_turn + 1;

    let label = if compaction.summary.is_some() {
        Some("summary".to_owned())
    } else {
        compaction_policy_label(compaction)
    };

    let text = match &label {
        Some(label) => format!("turns {from}..{to} ({count} total, {label})"),
        None => format!("turns {from}..{to} ({count} total)"),
    };

    DetailItem::new(
        text,
        json!({
            "from_turn": from,
            "to_turn": to,
            "reasoning": compaction.reasoning.is_some(),
            "tool_calls": compaction.tool_calls.as_ref(),
            "summary": compaction.summary.as_ref().map(|s| &s.summary),
        }),
    )
}

/// Describe a compaction's mechanical policies (reasoning / tool calls), e.g.
/// `reasoning + tools`.
///
/// Summaries take precedence over mechanical policies and are labeled
/// separately by the caller.
/// Returns `None` when the compaction carries no mechanical policy.
pub(crate) fn compaction_policy_label(compaction: &Compaction) -> Option<String> {
    let mut parts = Vec::new();
    if compaction.reasoning.is_some() {
        parts.push("reasoning");
    }
    if let Some(policy) = &compaction.tool_calls {
        match policy {
            ToolCallPolicy::Strip {
                request: true,
                response: true,
            } => parts.push("tools"),
            ToolCallPolicy::Strip {
                request: true,
                response: false,
            } => parts.push("tool requests"),
            ToolCallPolicy::Strip {
                request: false,
                response: true,
            } => parts.push("tool responses"),
            ToolCallPolicy::Strip {
                request: false,
                response: false,
            } => {}
            ToolCallPolicy::Omit => parts.push("tools omitted"),
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" + "))
    }
}

/// Convert a [`Color`] to an SGR background parameter string.
pub(crate) fn color_to_bg_param(color: Color) -> String {
    match color {
        Color::Ansi256(n) => format!("48;5;{n}"),
        Color::Rgb { r, g, b } => format!("48;2;{r};{g};{b}"),
    }
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
