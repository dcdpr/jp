use serde_json::json;

use super::*;

#[test]
fn the_answer_is_the_text_after_the_last_tool_call() {
    let events = vec![
        json!({ "type": "turn_start" }),
        json!({ "type": "chat_request", "content": "What is being worked on?" }),
        json!({ "type": "chat_response", "message": "Let me look." }),
        json!({ "type": "tool_call_request", "id": "1", "name": "ticket_list" }),
        json!({ "type": "tool_call_response", "id": "1", "content": "..." }),
        json!({ "type": "chat_response", "reasoning": "Two tickets are in progress." }),
        json!({ "type": "chat_response", "message": "Two things:\n\n" }),
        json!({ "type": "chat_response", "message": "- T-1e932pb" }),
    ];

    assert_eq!(
        last_answer(&events),
        Some("Two things:\n\n- T-1e932pb".to_owned())
    );
}

#[test]
fn only_the_last_turn_counts() {
    let events = vec![
        json!({ "type": "turn_start" }),
        json!({ "type": "chat_response", "message": "Yesterday's answer." }),
        json!({ "type": "turn_start" }),
        json!({ "type": "chat_request", "content": "And now?" }),
        json!({ "type": "chat_response", "message": "Today's answer." }),
    ];

    assert_eq!(last_answer(&events), Some("Today's answer.".to_owned()));
}

#[test]
fn a_turn_ending_on_a_tool_call_answers_with_its_last_text() {
    let events = vec![
        json!({ "type": "turn_start" }),
        json!({ "type": "chat_response", "message": "Filing it now." }),
        json!({ "type": "tool_call_request", "id": "1", "name": "ticket_create" }),
        json!({ "type": "tool_call_response", "id": "1", "content": "Created" }),
    ];

    assert_eq!(last_answer(&events), Some("Filing it now.".to_owned()));
}

#[test]
fn a_turn_without_text_has_no_answer() {
    let events = vec![
        json!({ "type": "turn_start" }),
        json!({ "type": "chat_request", "content": "Hi" }),
        json!({ "type": "chat_response", "reasoning": "Thinking." }),
    ];

    assert_eq!(last_answer(&events), None);
}
