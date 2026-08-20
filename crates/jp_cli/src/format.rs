pub(crate) mod conversation;
pub(crate) mod datetime;

use jp_config::types::color::Color;
use jp_conversation::{ByteSize, Compaction, ToolCallPolicy};
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

/// Render a label the way the user writes it: `key=value`, or the bare key when
/// the value is empty.
pub(crate) fn label_text(key: &str, value: &str) -> String {
    if value.is_empty() {
        key.to_owned()
    } else {
        format!("{key}={value}")
    }
}

/// Build a list item for a conversation label.
///
/// The terminal text reads as `key=value`, or as the bare key when the label
/// carries an empty value.
/// The JSON form is always an object with `key` and `value`.
pub(crate) fn label_detail_item(key: &str, value: &str) -> DetailItem {
    DetailItem::new(
        label_text(key, value),
        json!({ "key": key, "value": value }),
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
/// `reasoning` and `tool_calls` mirror their own serialized shapes (e.g.
/// `{"policy": "strip", "request": true, "response": true}`) rather than the
/// `--tools` flag vocabulary, since a policy can carry `request`/`response`
/// combinations the flag can't express, plus an `over` size threshold.
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
            "reasoning": compaction.reasoning.as_ref(),
            "tool_calls": compaction.tool_calls.as_ref(),
            "summary": compaction.summary.as_ref().map(|s| &s.summary),
        }),
    )
}

/// Describe a compaction's mechanical policies (reasoning / tool calls), e.g.
/// `reasoning + tools`.
///
/// A policy narrowed by a size threshold reads as `tool responses over 1MB`, so
/// the label distinguishes a rule that compacted everything in its range from
/// one that only reached the large items.
///
/// Summaries take precedence over mechanical policies and are labeled
/// separately by the caller.
/// Returns `None` when the compaction carries no mechanical policy.
pub(crate) fn compaction_policy_label(compaction: &Compaction) -> Option<String> {
    /// Append a policy's threshold, when it has one.
    fn qualified(name: &str, over: Option<ByteSize>) -> String {
        over.map_or_else(|| name.to_owned(), |size| format!("{name} over {size}"))
    }

    let mut parts = Vec::new();
    if let Some(spec) = &compaction.reasoning {
        parts.push(qualified("reasoning", spec.over));
    }
    if let Some(spec) = &compaction.tool_calls {
        let name = match spec.policy {
            ToolCallPolicy::Strip {
                request: true,
                response: true,
            } => Some("tools"),
            ToolCallPolicy::Strip {
                request: true,
                response: false,
            } => Some("tool requests"),
            ToolCallPolicy::Strip {
                request: false,
                response: true,
            } => Some("tool responses"),
            ToolCallPolicy::Strip {
                request: false,
                response: false,
            } => None,
            ToolCallPolicy::Omit => Some("tools omitted"),
        };
        if let Some(name) = name {
            parts.push(qualified(name, spec.over));
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
