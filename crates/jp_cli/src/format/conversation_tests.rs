use std::time::Duration;

use chrono::{DateTime, Utc};
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

#[test]
fn details_show_the_status_beneath_the_title_and_the_paths_last() {
    let details = DetailsFmt::new(make_id(1000))
        .with_title(Some("Fix the loader"))
        .with_status(Some("unable to load: missing events.json"))
        .with_paths(vec![
            "/ws/.jp/conversations/10000".into(),
            "/user/conversations/10000-fix-the-loader".into(),
        ])
        .with_pretty_printing(false);

    let expected = [
        "     ID  jp-c10000",
        "  Title  Fix the loader",
        " Status  unable to load: missing events.json",
        "  Paths",
        "         1. /ws/.jp/conversations/10000",
        "         2. /user/conversations/10000-fix-the-loader",
    ];
    assert_eq!(details.to_string(), expected.join("\n"));
}

/// The payload keeps every key present, so a consumer reads `model: null`
/// rather than a missing key when no model was set.
#[test]
fn json_reports_a_missing_model_as_null() {
    let details = DetailsFmt::new(make_id(1000));

    let json = details.json();

    assert_eq!(json.get("model"), Some(&serde_json::Value::Null));
}
