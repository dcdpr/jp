use crate::message::*;

#[test]
fn conversations_response_serializes_without_null_id() {
    let resp = HostToPlugin::Conversations(ConversationsResponse {
        id: None,
        data: vec![ConversationSummary {
            id: "123".to_owned(),
            title: Some("Test".to_owned()),
            last_activated_at: chrono::Utc::now(),
            pinned_at: None,
            events_count: 5,
        }],
    });

    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(
        json.get("id"),
        None,
        "an absent id must be omitted, not null"
    );
}
