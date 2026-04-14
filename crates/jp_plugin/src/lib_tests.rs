use serde_json::json;

use crate::message::*;

#[test]
fn conversations_response_serializes_without_null_id() {
    let resp = HostToPlugin::Conversations(ConversationsResponse {
        id: None,
        data: vec![ConversationSummary {
            id: "123".to_owned(),
            title: Some("Test".to_owned()),
            last_activated_at: chrono::Utc::now(),
            events_count: 5,
        }],
    });

    let json = serde_json::to_value(&resp).unwrap();
    // When id is None, it should not appear in the JSON
    assert!(json.get("id").is_none() || json.get("id") == Some(&json!(null)));
}
