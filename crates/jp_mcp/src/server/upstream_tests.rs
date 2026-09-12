use jp_tool::Outcome;
use serde_json::json;

use super::*;
use crate::Content;

#[test]
fn single_text_outcome_is_unwrapped_once() {
    let text = r#"{"type":"success","content":"{\"type\":\"success\",\"content\":\"nested\"}"}"#;
    let output = decode_result(CallToolResult::success(vec![Content::text(text)])).unwrap();
    let UpstreamResult::Outcome {
        outcome: Outcome::Success { content },
        ..
    } = output
    else {
        panic!("expected Outcome")
    };
    assert_eq!(content, r#"{"type":"success","content":"nested"}"#);
}

#[test]
fn mixed_content_and_metadata_are_preserved() {
    let input: CallToolResult = serde_json::from_value(json!({
        "content":[{"type":"text","text":"{\"type\":\"success\",\"content\":\"plain\"}"},{"type":"image","data":"AA==","mimeType":"image/png"}],
        "isError":false,"structuredContent":{"answer":42},"_meta":{"custom":"retained"}
    })).unwrap();
    let expected = serde_json::to_value(&input).unwrap();
    let UpstreamResult::Native(result) = decode_result(input).unwrap() else {
        panic!("expected native result")
    };
    assert_eq!(serde_json::to_value(result).unwrap(), expected);
}

#[test]
fn mcp_error_flag_wins_over_success_envelope() {
    let input = CallToolResult::error(vec![Content::text(
        r#"{"type":"success","content":"done"}"#,
    )]);
    let UpstreamResult::Native(result) = decode_result(input).unwrap() else {
        panic!("expected native error")
    };
    assert_eq!(result.is_error, Some(true));
    assert_eq!(result.content, vec![Content::text(
        r#"{"type":"success","content":"done"}"#
    )]);
}

#[test]
fn malformed_recognized_inquiry_is_not_plain_output() {
    assert!(
        decode_result(CallToolResult::success(vec![Content::text(
            r#"{"type":"needs_input","question":{"id":"bad.id"}}"#
        )]))
        .is_err()
    );
}

#[test]
fn ordinary_text_is_native() {
    let UpstreamResult::Native(result) =
        decode_result(CallToolResult::success(vec![Content::text("hello")])).unwrap()
    else {
        panic!("expected text")
    };
    assert_eq!(result.content, vec![Content::text("hello")]);
}

#[test]
fn unwrapped_envelope_preserves_annotations_and_result_metadata() {
    let input: CallToolResult = serde_json::from_value(json!({
        "content":[{"type":"text","text":"{\"type\":\"success\",\"content\":\"done\"}","annotations":{"audience":["assistant"]}}],
        "structuredContent":{"answer":42},"_meta":{"custom":"retained"}
    })).unwrap();
    let UpstreamResult::Outcome {
        outcome: Outcome::Success { content },
        response,
    } = decode_result(input).unwrap()
    else {
        panic!("expected envelope")
    };
    let result = replace_envelope(response, &content, false);
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        json!({
            "content":[{"type":"text","text":"done","annotations":{"audience":["assistant"]}}],
            "isError":false,"structuredContent":{"answer":42},"_meta":{"custom":"retained"}
        })
    );
}

#[test]
fn unrelated_error_json_is_not_a_malformed_outcome() {
    let result = CallToolResult::success(vec![Content::text(r#"{"type":"error","code":42}"#)]);
    let UpstreamResult::Native(result) = decode_result(result).unwrap() else {
        panic!("expected native data")
    };
    assert_eq!(result.content, vec![Content::text(
        r#"{"type":"error","code":42}"#
    )]);
}
