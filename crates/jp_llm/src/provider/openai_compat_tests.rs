use serde_json::json;

use super::*;

/// A chunk carrying content parses and is handed back.
#[test]
fn parse_chunk_accepts_an_ordinary_chunk() {
    let data = r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#;

    let chunk = parse_chunk(data, "test").expect("chunk with choices is kept");

    assert_eq!(chunk.choices.len(), 1);
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("hi"));
}

#[test]
fn parse_chunk_drops_a_malformed_payload() {
    assert!(parse_chunk("{not json", "test").is_none());
}

/// A chunk with no choices yields no events, so there is nothing to hand back.
///
/// This is the benign shape: a server that reports usage in its own chunk sends
/// one, and it is not worth a warning.
#[test]
fn parse_chunk_drops_a_chunk_without_choices() {
    let data = r#"{"choices":[],"usage":{"total_tokens":7}}"#;

    assert!(parse_chunk(data, "test").is_none());
}

/// llama.cpp opens every stream with a role-only delta, and repeats it on each
/// progress update.
/// It carries a choice, but nothing a handler can emit.
#[test]
fn parse_chunk_drops_a_role_only_chunk() {
    let data = r#"{"choices":[{"finish_reason":null,"index":0,"delta":{"role":"assistant","content":null}}]}"#;

    assert!(parse_chunk(data, "test").is_none());
}

/// An error reported inside the stream is the shape worth noticing.
///
/// The chunk types ignore unknown fields, so before `error` was captured this
/// payload deserialized into a chunk with an empty `choices` and was
/// indistinguishable from the benign case above.
#[test]
fn parse_chunk_captures_an_in_stream_error() {
    let data = r#"{"error":{"message":"upstream exploded","type":"server_error"}}"#;

    let chunk: StreamChunk = serde_json::from_str(data).expect("chunk parses");

    assert_eq!(
        chunk.error,
        Some(json!({"message":"upstream exploded","type":"server_error"}))
    );
    assert!(parse_chunk(data, "test").is_none());
}

/// Capturing `error` must not disturb the ordinary path.
#[test]
fn an_ordinary_chunk_carries_no_error() {
    let data = r#"{"choices":[{"delta":{"content":"hi"},"index":0}]}"#;

    let chunk: StreamChunk = serde_json::from_str(data).expect("chunk parses");

    assert_eq!(chunk.error, None);
}
