use jp_tool::{Outcome, Question};
use serde_json::json;

use super::*;

#[test]
fn native_content_round_trips_through_shared_result() {
    let wire = json!({
        "content": [
            {"type":"text", "text":"first", "_meta":{"vendor":"text"}, "annotations":{"audience":["assistant"],"priority":0.5}},
            {"type":"image", "data":"aW1hZ2U=", "mimeType":"image/png", "_meta":{"vendor":"image"}},
            {"type":"audio", "data":"YXVkaW8=", "mimeType":"audio/wav"},
            {"type":"resource", "resource":{"uri":"file:///a", "text":"embedded", "mimeType":"text/plain", "_meta":{"inner":true}}, "_meta":{"outer":true}},
            {"type":"resource", "resource":{"uri":"file:///b", "blob":"YmxvYg=="}},
            {"type":"resource_link", "uri":"file:///c", "name":"c", "size":42, "icons":[{"src":"file:///icon", "theme":"dark"}]}
        ],
        "structuredContent":{"number":42},
        "_meta":{"vendor":{"preserved":true}},
        "isError":false
    });
    let native = serde_json::from_value(wire.clone()).unwrap();
    let result = from_mcp(native).unwrap();
    assert_eq!(result.content.len(), 6);
    assert!(matches!(result.content[1], ContentBlock::Image(_)));
    assert_eq!(
        to_legacy(&result),
        Ok("first\n\nembedded\n\nYmxvYg==".into())
    );
    assert_eq!(serde_json::to_value(to_mcp(result).unwrap()).unwrap(), wire);
}

#[test]
fn error_details_survive_mcp_encoding() {
    let result = ToolResult::from(Outcome::Error {
        message: "busy".into(),
        trace: vec!["upstream".into()],
        transient: true,
    });
    let native = to_mcp(result.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(&native).unwrap(),
        json!({
            "content":[{"type":"text","text":"busy\n\nTrace:\nupstream"}],
            "isError":true,
            "_meta":{"computer.jp/error":{"transient":true,"trace":["upstream"]}}
        })
    );
    let decoded = from_mcp(native).unwrap();
    assert_eq!(decoded.status, result.status);
    assert_eq!(to_legacy(&decoded), Err("busy\n\nTrace:\nupstream".into()));
}

#[test]
fn unresolved_input_cannot_be_sent_as_final_output() {
    let result = ToolResult::from(Outcome::NeedsInput {
        question: Question::boolean("confirm", "Proceed?").unwrap(),
    });
    assert!(matches!(
        to_mcp(result),
        Err(ResultError::UnansweredQuestion)
    ));
}

#[test]
fn error_metadata_extensions_and_empty_audience_survive() {
    let wire = json!({
        "content":[{"type":"text","text":"failed", "annotations":{"audience":[]}}],
        "isError":true,
        "_meta":{"computer.jp/error":{"transient":false,"trace":[],"vendorCode":17}}
    });
    let result = from_mcp(serde_json::from_value(wire.clone()).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(to_mcp(result).unwrap()).unwrap(), wire);
}

#[test]
fn malformed_error_metadata_is_rejected() {
    let wire = json!({"content":[], "isError":true, "_meta":{"computer.jp/error":{"transient":"yes", "trace":[]}}});
    let error = from_mcp(serde_json::from_value(wire).unwrap()).unwrap_err();
    assert!(error.is_data());
}

#[test]
fn omitted_status_is_preserved() {
    let wire = json!({"content":[]});
    let result = from_mcp(serde_json::from_value(wire.clone()).unwrap()).unwrap();
    assert_eq!(result.status, ToolStatus::Unspecified);
    assert_eq!(serde_json::to_value(to_mcp(result).unwrap()).unwrap(), wire);
}
