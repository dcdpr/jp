use chrono::TimeZone as _;

use super::*;

#[test]
fn test_conversation_serialization() {
    let conv = Conversation {
        title: None,
        last_activated_at: Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
        user: true,
        pinned: false,
        expires_at: None,
        last_event_at: None,
        events_count: 0,
    };

    insta::assert_json_snapshot!(conv);
}
