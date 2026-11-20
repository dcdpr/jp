use std::time::Duration;

use chrono::{DateTime, Utc};
use jp_storage::validate::{ConversationPart, StorageLocation, ValidationError};
use serde_json::json;

use super::*;

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(secs)).unwrap()
}

#[test]
fn details_show_the_model_beneath_the_title() {
    let details = DetailsFmt::new(make_id(1000))
        .with_title(Some("Rework the config pipeline"))
        .with_model("anthropic/claude-sonnet-4-5")
        .with_pretty_printing(false);

    assert_eq!(
        details.to_string(),
        "    ID  jp-c10000\n Title  Rework the config pipeline\n Model  \
         anthropic/claude-sonnet-4-5"
    );
    assert_eq!(
        details.json()["model"],
        json!("anthropic/claude-sonnet-4-5")
    );
}

fn missing_events() -> ValidationError {
    ValidationError::Missing {
        part: ConversationPart::Events,
        location: StorageLocation::new("/ws/.jp/conversations/10000/events.json"),
    }
}

#[test]
fn details_show_the_status_beneath_the_title_and_the_locations_last() {
    let details = DetailsFmt::new(make_id(1000))
        .with_title(Some("Fix the loader"))
        .with_status(Some(missing_events()))
        .with_locations(vec![
            StorageLocation::new("/ws/.jp/conversations/10000"),
            StorageLocation::new("/user/conversations/10000-fix-the-loader"),
        ])
        .with_pretty_printing(false);

    let expected = [
        "        ID  jp-c10000",
        "     Title  Fix the loader",
        "    Status  unable to load: missing events",
        " Locations",
        "            1. /ws/.jp/conversations/10000",
        "            2. /user/conversations/10000-fix-the-loader",
    ];
    assert_eq!(details.to_string(), expected.join("\n"));
}

// A conversation `jp c show` describes loads, so its status is `null`; the
// locations are where to find its files.
#[test]
fn json_lists_the_locations_and_a_null_status_for_a_conversation_that_loads() {
    let details = DetailsFmt::new(make_id(1000))
        .with_locations(vec![StorageLocation::new("/ws/.jp/conversations/10000")]);

    let json = details.json();

    assert_eq!(json["locations"], json!(["/ws/.jp/conversations/10000"]));
    assert_eq!(json["status"], serde_json::Value::Null);
}

// The status is typed: the same code and causes the load-failure report emits.
#[test]
fn json_describes_why_a_conversation_does_not_load() {
    let details = DetailsFmt::new(make_id(1000)).with_status(Some(missing_events()));

    assert_eq!(
        details.json()["status"],
        json!({
            "code": "conversation_load_failed",
            "message": "conversation failed to load",
            "causes": [{
                "code": "part_missing",
                "message": "missing conversation part",
                "part": "events",
                "location": "/ws/.jp/conversations/10000/events.json",
            }],
        })
    );
}

/// The payload keeps every key present, so a consumer reads `model: null`
/// rather than a missing key when no model was set.
#[test]
fn json_reports_a_missing_model_as_null() {
    let details = DetailsFmt::new(make_id(1000));

    let json = details.json();

    assert_eq!(json.get("model"), Some(&serde_json::Value::Null));
}
