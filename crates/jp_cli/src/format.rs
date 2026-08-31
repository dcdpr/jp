pub(crate) mod conversation;
pub(crate) mod datetime;
pub(crate) mod workspace;

use indexmap::IndexSet;
use jp_config::types::color::Color;
use jp_conversation::{Compaction, Labels, ToolCallPolicy};
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

/// Render one label as an output line: `marker`, then the pair as the user
/// writes it.
///
/// Every line JP prints for a label carries a marker column, so a reader strips
/// one character and has the pair, whichever command produced the line.
/// A label key starts with a letter, so the column is never mistaken for part
/// of the label.
pub(crate) fn label_line(marker: char, key: &str, value: &str) -> String {
    format!("{marker}{}", label_text(key, value))
}

/// Build a list item for one label key and the values it holds.
///
/// The terminal text is the key, with one value per line beneath it, and the
/// bare key alone when it holds none.
/// Values are listed rather than comma-separated because a value may itself
/// contain a comma, which would make one value indistinguishable from two.
/// The JSON form is always an object with `key` and `values`.
pub(crate) fn label_detail_item(key: &str, values: &IndexSet<String>) -> DetailItem {
    let values = shown_values(values);
    let json = json!({ "key": key, "values": values });

    if values.is_empty() {
        return DetailItem::new(key, json);
    }

    let listed = values
        .iter()
        .map(|value| format!("    {value}"))
        .collect::<Vec<_>>()
        .join("\n");

    DetailItem::new(format!("{key}\n{listed}"), json)
}

/// The values of a label key that a reader can see.
///
/// A bare label is stored as the empty value, which is an encoding of the key's
/// presence rather than something to show: the key itself already says it.
pub(crate) fn shown_values(values: &IndexSet<String>) -> Vec<&str> {
    values
        .iter()
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .collect()
}

/// Build one list item per label key, sorted by key.
pub(crate) fn label_detail_items(labels: &Labels) -> Vec<DetailItem> {
    labels
        .iter()
        .map(|(key, values)| label_detail_item(key, values))
        .collect()
}

/// Render labels as one line per value, sorted by key.
///
/// Every line stands alone, so a reader needs no context from the lines around
/// it: a bare label is the key by itself, and a key holding several values
/// repeats across lines.
/// The marker column is a space, because a listing reports what is there rather
/// than a change to it.
pub(crate) fn label_lines(labels: &Labels) -> Vec<String> {
    labels
        .iter()
        .flat_map(|(key, values)| values.iter().map(move |value| label_line(' ', key, value)))
        .collect()
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
