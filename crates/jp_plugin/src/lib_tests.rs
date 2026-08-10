use serde_json::json;

use crate::{message::*, ready};

#[test]
fn the_handshake_refuses_a_host_that_is_too_old() {
    assert_eq!(ready(2, 2), Ok(ReadyMessage { protocol: 2 }));
    assert_eq!(ready(1, 2), Ok(ReadyMessage { protocol: 1 }));

    let exit = ready(2, 1).unwrap_err();
    assert_eq!(exit.code, 1);
    assert!(
        exit.reason
            .as_ref()
            .is_some_and(|reason| reason.contains("protocol 2") && reason.contains("speaks 1")),
        "{:?}",
        exit.reason
    );
}

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
