use serde_json::{from_str, to_string};

use crate::EventId;

#[test]
fn serde_roundtrip() {
    let id = EventId::fixed("k3m9x2a");

    let json = to_string(&id).expect("serialize event ID");
    assert_eq!(json, r#""k3m9x2a""#);
    assert_eq!(
        from_str::<EventId>(&json).expect("deserialize event ID"),
        id
    );
}

#[test]
fn deserialize_rejects_empty_string() {
    let error = from_str::<EventId>(r#""""#).expect_err("empty event ID must be rejected");

    assert!(error.to_string().contains("event ID must not be empty"));
}

#[test]
fn deserialize_rejects_non_strings() {
    for json in ["null", "7", "true", "[]", "{}"] {
        assert!(from_str::<EventId>(json).is_err(), "accepted {json}");
    }
}

#[test]
fn deserialize_preserves_non_format_id() {
    let id = from_str::<EventId>(r#""Hand-edited ID: 42!""#).expect("non-empty event ID");

    assert_eq!(id, EventId::fixed("Hand-edited ID: 42!"));
    assert_eq!(
        to_string(&id).expect("serialize event ID"),
        r#""Hand-edited ID: 42!""#
    );
}

#[test]
fn deserialize_preserves_unicode() {
    let id = from_str::<EventId>(r#""\u00e9\u2603""#).expect("non-empty event ID");

    assert_eq!(id, EventId::fixed("\u{00e9}\u{2603}"));
}

#[test]
fn deserialize_preserves_whitespace() {
    let id = from_str::<EventId>(r#"" \t\n ""#).expect("non-empty event ID");

    assert_eq!(id, EventId::fixed(" \t\n "));
    assert_eq!(to_string(&id).expect("serialize event ID"), r#"" \t\n ""#);
}

#[test]
fn display_and_debug() {
    let id = EventId::fixed("k3m9x2a");

    assert_eq!(id.to_string(), "k3m9x2a");
    assert_eq!(format!("{id:?}"), r#"EventId("k3m9x2a")"#);
}

#[test]
fn random_id_uses_generation_format() {
    let id = EventId::random().to_string();

    assert_eq!(id.len(), 7);
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    );
}

#[test]
fn random_calls_produce_distinct_ids() {
    assert_ne!(EventId::random(), EventId::random());
}

#[test]
#[should_panic(expected = "event ID must not be empty")]
fn fixed_id_rejects_empty_string() {
    let _id = EventId::fixed("");
}
