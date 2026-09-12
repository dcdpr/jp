//! JP-aware result decoding for upstream MCP tools.

use jp_tool::Outcome;
use serde_json::Value;
use tracing::warn;

use crate::{CallToolResult, RawContent};

pub(super) enum UpstreamResult {
    Outcome {
        outcome: Outcome,
        response: CallToolResult,
    },
    Native(CallToolResult),
}

/// Recognize one complete legacy envelope without flattening native content.
pub(super) fn decode_result(result: CallToolResult) -> Result<UpstreamResult, serde_json::Error> {
    if let [content] = result.content.as_slice()
        && let RawContent::Text(text) = &content.raw
    {
        match serde_json::from_str::<Outcome>(&text.text) {
            Ok(Outcome::Success { .. }) if result.is_error == Some(true) => {
                warn!("MCP error flag conflicts with an Outcome::Success envelope");
            }
            Ok(outcome) => {
                return Ok(UpstreamResult::Outcome {
                    outcome,
                    response: result,
                });
            }
            Err(error) => {
                let value = serde_json::from_str::<Value>(&text.text).ok();
                if matches!(
                    value
                        .as_ref()
                        .and_then(|v| v.get("type"))
                        .and_then(Value::as_str),
                    Some("needs_input")
                ) {
                    return Err(error);
                }
            }
        }
    }
    Ok(UpstreamResult::Native(result))
}

/// Replace an unwrapped envelope while retaining its native result metadata.
pub(super) fn replace_envelope(
    mut response: CallToolResult,
    text: &str,
    is_error: bool,
) -> CallToolResult {
    if let Some(content) = response.content.first_mut()
        && let RawContent::Text(content) = &mut content.raw
    {
        content.text = text.into();
    }
    response.is_error = Some(is_error);
    response
}

#[cfg(test)]
#[path = "upstream_tests.rs"]
mod tests;
