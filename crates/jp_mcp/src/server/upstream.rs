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
///
/// Only a result that is exactly one text block is a candidate, and only if
/// that text parses whole.
/// Anything else is a native MCP result and is returned untouched, mixed
/// content included.
pub(super) fn decode_result(result: CallToolResult) -> Result<UpstreamResult, serde_json::Error> {
    let [content] = result.content.as_slice() else {
        return Ok(UpstreamResult::Native(result));
    };
    let RawContent::Text(text) = &content.raw else {
        return Ok(UpstreamResult::Native(result));
    };

    match serde_json::from_str::<Outcome>(&text.text) {
        // The server said the call failed and its payload says it succeeded.
        // Per RFD 108 the flag wins, so the envelope is left unrecognized and
        // the failure carries through as the native result it already is.
        Ok(Outcome::Success { .. }) if result.is_error == Some(true) => {
            warn!("MCP error flag conflicts with an Outcome::Success envelope");
            Ok(UpstreamResult::Native(result))
        }
        Ok(outcome) => Ok(UpstreamResult::Outcome {
            outcome,
            response: result,
        }),
        // A payload shaped like an inquiry that will not parse is a protocol
        // mismatch, not prose: handing the raw JSON to the model would hide it.
        Err(error) if is_needs_input(&text.text) => Err(error),
        Err(_) => Ok(UpstreamResult::Native(result)),
    }
}

/// Whether the text is a JSON object announcing itself as an inquiry.
fn is_needs_input(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .as_ref()
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        == Some("needs_input")
}

/// Replace a recognized envelope's text while retaining its native metadata.
///
/// `response` is the result [`decode_result`] recognized, so its single text
/// block is the envelope being unwrapped.
pub(super) fn replace_envelope(
    mut response: CallToolResult,
    text: &str,
    is_error: bool,
) -> CallToolResult {
    debug_assert!(
        matches!(response.content.as_slice(), [content] if matches!(content.raw, RawContent::Text(_))),
        "only a recognized single-text envelope can be replaced"
    );
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
