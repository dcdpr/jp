use serde_json::{Map, Value};

use super::{
    ChatRequest, ChatResponse, EventKind, InquiryId, InquiryQuestion, InquiryRequest,
    InquiryResponse, InquirySource, ToolCallRequest, ToolCallResponse, TurnStart,
};

/// Fails to compile when a variant is added to [`EventKind`].
///
/// That is the reminder to extend [`every_kind`] below and
/// [`EventKind::TYPE_TAGS`].
/// Neither is checked by anything else, and the two tests here agree with each
/// other when a variant is missing from both: the tags they compare are derived
/// from `every_kind`, so a variant absent there is absent from both sides.
/// A variant missing from `TYPE_TAGS` is unreachable on load, and the stream
/// keeps every one of its events as raw JSON instead.
#[expect(dead_code)]
fn every_variant_is_listed(kind: &EventKind) {
    match kind {
        EventKind::TurnStart(_)
        | EventKind::ChatRequest(_)
        | EventKind::ChatResponse(_)
        | EventKind::ToolCallRequest(_)
        | EventKind::ToolCallResponse(_)
        | EventKind::InquiryRequest(_)
        | EventKind::InquiryResponse(_) => {}
    }
}

/// One value of every variant.
fn every_kind() -> Vec<EventKind> {
    vec![
        TurnStart.into(),
        ChatRequest::from("hi").into(),
        ChatResponse::message("hello").into(),
        ToolCallRequest::new("call-1".to_owned(), "read_file".to_owned(), Map::new()).into(),
        ToolCallResponse {
            id: "call-1".to_owned(),
            result: Ok("contents".to_owned()),
        }
        .into(),
        InquiryRequest::new(
            InquiryId::new("q1"),
            InquirySource::User,
            InquiryQuestion::text("Which file?".to_owned()),
        )
        .into(),
        InquiryResponse::boolean(InquiryId::new("q1"), true).into(),
    ]
}

/// The tag a variant serializes as is what every reader outside Rust switches
/// on, so it has to be the tag serde actually writes rather than a name kept
/// alongside it by hand.
#[test]
fn every_variants_tag_is_the_one_serde_writes() {
    for kind in every_kind() {
        let serialized = serde_json::to_value(&kind).expect("serializes");
        let written = serialized
            .get("type")
            .and_then(Value::as_str)
            .expect("carries a type tag");

        assert_eq!(kind.type_tag(), written, "for {}", kind.as_str());
    }
}

/// The deserializer decides whether an entry is a known event by looking its
/// tag up in this list, so a tag missing from it makes the variant unreachable:
/// the stream would keep every one of those events as raw JSON instead.
#[test]
fn every_variants_tag_is_listed_as_recognized() {
    let tags: Vec<&str> = every_kind().iter().map(EventKind::type_tag).collect();

    assert_eq!(tags, EventKind::TYPE_TAGS);
}
