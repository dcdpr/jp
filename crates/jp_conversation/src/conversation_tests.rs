use chrono::TimeZone as _;

use super::*;

#[test]
fn test_conversation_serialization() {
    let conv = Conversation {
        title: None,
        last_activated_at: Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
        pinned_at: None,
        archived_at: None,
        expires_at: None,
        labels: Labels::default(),
        last_event_at: None,
        events_count: 0,
    };

    insta::assert_json_snapshot!(conv);
}

/// Conversations written before labels existed have no `labels` key; they must
/// keep loading with an empty set rather than failing to parse.
#[test]
fn missing_labels_field_loads_as_empty() {
    let json = r#"{"last_activated_at":"2023-01-01T00:00:00Z"}"#;

    let conv: Conversation = serde_json::from_str(json).unwrap();

    assert!(conv.labels.is_empty());
}

/// A conversation written before labels held sets carries scalars; it loads
/// unchanged and is written back as arrays.
#[test]
fn labels_round_trip_through_metadata() {
    let json =
        r#"{"last_activated_at":"2023-01-01 00:00:00.0","labels":{"branch":"main","draft":""}}"#;

    let conv: Conversation = serde_json::from_str(json).unwrap();

    assert!(conv.labels.contains("branch", "main"));
    assert!(conv.labels.contains("draft", ""));
    assert_eq!(
        serde_json::to_string(&conv).unwrap(),
        r#"{"last_activated_at":"2023-01-01 00:00:00.0","labels":{"branch":["main"],"draft":[""]}}"#
    );
}

/// A field the reader does not know is skipped rather than failing the load.
///
/// That is what lets a JP built before a field existed read a conversation
/// written after it: `labels` is simply not there for it, whatever shape the
/// values take.
#[test]
fn an_unknown_field_is_ignored() {
    let json = r#"{"last_activated_at":"2023-01-01 00:00:00.0","labels":{"crate":["jp_config"]},"from_the_future":{"a":1}}"#;

    let conv: Conversation = serde_json::from_str(json).unwrap();

    assert!(conv.labels.contains("crate", "jp_config"));
}
