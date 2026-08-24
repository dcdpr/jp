use std::slice;

use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ChatResponse},
};

use super::{EventMatcher, EventPatch, PatchAction, apply_patches};

/// A stream holding one assistant response per `(key, value)` metadata pair.
///
/// The two leading entries in every expectation below are the turn scaffold
/// (`TurnStart` and the `ChatRequest`), which carry no metadata.
fn stream_with_metadata(entries: &[(&str, &str)]) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("a request"));
    stream.extend(entries.iter().map(|(key, value)| {
        ConversationEvent::now(ChatResponse::message("a response"))
            .with_metadata_field(*key, *value)
    }));
    stream
}

/// Remove `key` from every event whose `key` equals `value`.
fn remove_matching(key: &str, value: &str) -> EventPatch {
    EventPatch {
        matcher: EventMatcher::MetadataValue {
            key: key.to_owned(),
            value: value.to_owned(),
        },
        action: PatchAction::RemoveMetadata(key.to_owned()),
    }
}

/// The metadata keys left on each event, in stream order.
fn metadata_keys(stream: &ConversationStream) -> Vec<String> {
    stream
        .iter()
        .map(|e| {
            e.event
                .metadata
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect()
}

#[test]
fn removing_a_matched_field_counts_once() {
    let mut stream = stream_with_metadata(&[("signature", "stale")]);
    let count = apply_patches(&mut stream, &[remove_matching("signature", "stale")]);

    assert_eq!(count, 1);
    assert_eq!(metadata_keys(&stream), vec!["", "", ""]);
}

#[test]
fn a_patch_matching_nothing_counts_zero() {
    let mut stream = stream_with_metadata(&[("signature", "fresh")]);
    let count = apply_patches(&mut stream, &[remove_matching("signature", "stale")]);

    assert_eq!(count, 0);
    assert_eq!(metadata_keys(&stream), vec!["", "", "signature"]);
}

#[test]
fn a_match_that_removes_an_absent_field_counts_zero() {
    // The matcher key and the action key are independent, so a patch can select
    // an event and then remove nothing. Reporting that as progress would let a
    // caller resend an unchanged request forever.
    let mut stream = stream_with_metadata(&[("signature", "stale")]);
    let patch = EventPatch {
        matcher: EventMatcher::MetadataValue {
            key: "signature".to_owned(),
            value: "stale".to_owned(),
        },
        action: PatchAction::RemoveMetadata("some_other_key".to_owned()),
    };

    let count = apply_patches(&mut stream, &[patch]);

    assert_eq!(count, 0);
    assert_eq!(metadata_keys(&stream), vec!["", "", "signature"]);
}

#[test]
fn each_patch_removes_only_its_own_match() {
    // Providers degrade one signature per retry, so a second pass must find the
    // second signature still present and untouched by the first pass.
    let mut stream = stream_with_metadata(&[("signature", "one"), ("signature", "two")]);

    let first = apply_patches(&mut stream, &[remove_matching("signature", "one")]);
    assert_eq!(first, 1);
    assert_eq!(metadata_keys(&stream), vec!["", "", "", "signature"]);

    let second = apply_patches(&mut stream, &[remove_matching("signature", "two")]);
    assert_eq!(second, 1);
    assert_eq!(metadata_keys(&stream), vec!["", "", "", ""]);
}

#[test]
fn re_applying_a_spent_patch_counts_zero() {
    // The termination argument for the summarizer's retry loop: once a removal
    // has happened, the same patch can never report progress again.
    let mut stream = stream_with_metadata(&[("signature", "stale")]);
    let patch = remove_matching("signature", "stale");

    assert_eq!(apply_patches(&mut stream, slice::from_ref(&patch)), 1);
    assert_eq!(apply_patches(&mut stream, slice::from_ref(&patch)), 0);
}
