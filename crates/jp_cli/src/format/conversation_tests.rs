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

/// The payload keeps every key present, so a consumer reads `model: null`
/// rather than a missing key when no model was set.
#[test]
fn json_reports_a_missing_model_as_null() {
    let details = DetailsFmt::new(make_id(1000));

    let json = details.json();

    assert_eq!(json.get("model"), Some(&serde_json::Value::Null));
}
