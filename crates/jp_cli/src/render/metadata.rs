//! Typed accessors for renderer-specific event metadata.
//!
//! Wraps the stringly-typed `ConversationEvent::metadata` field with
//! encode/decode helpers so callers don't deal with raw keys or base64.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use jp_conversation::event::{ConversationEvent, RENDERED_ARGUMENTS_KEY};

/// Store rendered custom-formatter output on a `ToolCallRequest` event.
///
/// The content is base64-encoded to avoid JSON-escaping issues with
/// arbitrary terminal output.
pub fn set_rendered_arguments(event: &mut ConversationEvent, content: &str) {
    let encoded = STANDARD.encode(content.as_bytes());
    event.add_metadata_field(RENDERED_ARGUMENTS_KEY, encoded);
}

/// Read rendered custom-formatter output from a `ToolCallRequest` event.
///
/// Returns `None` if the metadata key is absent or the value can't be decoded.
pub fn get_rendered_arguments(event: &ConversationEvent) -> Option<String> {
    let encoded = event.metadata.get(RENDERED_ARGUMENTS_KEY)?.as_str()?;
    let bytes = STANDARD.decode(encoded).ok()?;
    String::from_utf8(bytes).ok()
}
