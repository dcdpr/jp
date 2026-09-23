//! The conversation projection has to read a real snapshot and still catch a
//! real difference.

use serde_json::{Value, json};

use super::{project_conversation, snapshot_body};

/// A tool round as a route records it, with the parts two runs never share
/// filled in per route.
fn tool_round(call_id: &str, arguments: &Value, answer: &str) -> Value {
    json!({
        "base_config": {},
        "events": [
            {
                "timestamp": "2020-01-01 00:00:00.0",
                "type": "chat_request",
                "content": "Please run the tool."
            },
            {
                "type": "config_delta",
                "timestamp": "2020-01-01 00:00:00.0",
                "delta": { "assistant": { "model": { "id": { "name": "gpt-5.6-luna" } } } }
            },
            {
                "timestamp": "2020-01-01 00:00:00.0",
                "type": "tool_call_request",
                "id": call_id,
                "name": "run_me",
                "arguments": arguments
            },
            {
                "timestamp": "2020-01-01 00:00:00.0",
                "type": "tool_call_response",
                "id": call_id,
                "content": "d29ya2luZyE=",
                "is_error": false
            },
            {
                "timestamp": "2020-01-01 00:00:00.0",
                "type": "chat_response",
                "reasoning": "thinking it over"
            },
            {
                "timestamp": "2020-01-01 00:00:00.0",
                "type": "chat_response",
                "message": answer,
                "metadata": { "openai_item_id": "bXNn" }
            }
        ]
    })
}

#[test]
fn the_same_round_in_two_runs_compares_equal() {
    let api = tool_round("call_aaa", &json!({ "bar": "Zm9v", "foo": null }), "Done.");
    let subscription = tool_round(
        "call_zzz",
        &json!({ "bar": "Zm9v", "foo": "Zm9v" }),
        "The tool ran.",
    );

    assert_eq!(
        project_conversation(&api),
        project_conversation(&subscription)
    );
}

#[test]
fn the_projection_keeps_the_structure() {
    let projected = project_conversation(&tool_round("call_aaa", &json!({}), "Done."));

    assert_eq!(
        projected,
        json!([
            { "type": "chat_request", "content": "Please run the tool." },
            { "type": "tool_call_request", "id": "#0", "name": "run_me" },
            {
                "type": "tool_call_response",
                "id": "#0",
                "content": "d29ya2luZyE=",
                "is_error": false
            },
            { "type": "chat_response", "variant": "message" }
        ])
    );
}

#[test]
fn a_dropped_tool_result_is_caught() {
    let api = tool_round("call_aaa", &json!({}), "Done.");
    let mut subscription = tool_round("call_zzz", &json!({}), "Done.");
    subscription["events"]
        .as_array_mut()
        .unwrap()
        .retain(|event| event["type"] != "tool_call_response");

    assert_ne!(
        project_conversation(&api),
        project_conversation(&subscription)
    );
}

#[test]
fn a_renamed_tool_is_caught() {
    let api = tool_round("call_aaa", &json!({}), "Done.");
    let mut subscription = tool_round("call_zzz", &json!({}), "Done.");
    subscription["events"][2]["name"] = json!("something_else");

    assert_ne!(
        project_conversation(&api),
        project_conversation(&subscription)
    );
}

#[test]
fn a_failed_tool_result_is_caught() {
    let api = tool_round("call_aaa", &json!({}), "Done.");
    let mut subscription = tool_round("call_zzz", &json!({}), "Done.");
    subscription["events"][3]["is_error"] = json!(true);

    assert_ne!(
        project_conversation(&api),
        project_conversation(&subscription)
    );
}

/// The body starts after the second `---`, not the first.
#[test]
fn the_snapshot_header_is_stripped_whole() {
    let raw = "---\nsource: crates/jp_test/src/mock.rs\nexpression: v\n---\n{ \"events\": [] }\n";

    assert_eq!(snapshot_body(raw), Some("{ \"events\": [] }\n"));
}

/// A Windows checkout converts the fixtures to CRLF, and the body has to read
/// the same way it does on every other platform.
#[test]
fn a_crlf_snapshot_reads_like_an_lf_one() {
    let lf =
        "---\nsource: crates/jp_test/src/mock.rs\nexpression: v\n---\n{\n  \"events\": [\n    { \
         \"type\": \"chat_request\", \"content\": \"hi\" }\n  ]\n}\n";
    let crlf = lf.replace('\n', "\r\n");

    let read = |raw: &str| -> Value {
        project_conversation(&serde_json::from_str(snapshot_body(raw).unwrap()).unwrap())
    };

    assert_eq!(read(&crlf), read(lf));
    assert_eq!(
        read(lf),
        json!([{ "type": "chat_request", "content": "hi" }])
    );
}

/// A file that does not open with the header is not a snapshot.
#[test]
fn a_body_without_a_header_is_rejected() {
    assert_eq!(snapshot_body("{ \"events\": [] }\n---\n"), None);
}
