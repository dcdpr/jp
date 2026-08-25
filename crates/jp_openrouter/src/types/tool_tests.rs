use serde_json::json;

use super::*;

#[test]
fn serializes_function_call_type() {
    let call = ToolCall {
        id: Some("call_1".to_owned()),
        index: 0,
        kind: ToolCallType::Function,
        function: FunctionCall {
            name: Some("read_file".to_owned()),
            arguments: Some("{}".to_owned()),
        },
    };

    assert_eq!(
        serde_json::to_value(call).unwrap(),
        json!({
            "id": "call_1",
            "index": 0,
            "type": "function",
            "function": {
                "name": "read_file",
                "arguments": "{}"
            }
        })
    );
}

#[test]
fn missing_function_call_type_defaults_to_function() {
    let call: ToolCall = serde_json::from_value(json!({
        "id": "call_1",
        "index": 0,
        "function": {
            "name": "read_file",
            "arguments": "{}"
        }
    }))
    .unwrap();

    assert_eq!(call.kind, ToolCallType::Function);
}
