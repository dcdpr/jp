use std::str::FromStr as _;

use serde_json::{from_str, to_string};

use super::{EventIds, GENERATED_LEN};
use crate::{Error, EventId};

#[test]
fn new_accepts_a_non_empty_string() {
    assert_eq!(EventId::new("k3m9x2a").unwrap(), EventId::fixed("k3m9x2a"));
}

#[test]
fn new_rejects_an_empty_string() {
    assert!(matches!(EventId::new(""), Err(Error::EmptyEventId)));
}

#[test]
fn new_accepts_a_value_outside_the_generated_format() {
    // The format is a generation convention, so a caller naming an ID a
    // hand-edited file holds is not held to it.
    assert!(EventId::new("Hand-edited ID: 42!").is_ok());
}

#[test]
fn parsing_from_a_str_matches_new() {
    assert_eq!(
        EventId::from_str("k3m9x2a").unwrap(),
        EventId::new("k3m9x2a").unwrap()
    );
    assert!(matches!(EventId::from_str(""), Err(Error::EmptyEventId)));
}

#[test]
fn serializes_as_a_bare_string() {
    let id = EventId::fixed("k3m9x2a");

    assert_eq!(to_string(&id).unwrap(), r#""k3m9x2a""#);
}

#[test]
fn deserializes_from_a_bare_string() {
    assert_eq!(
        from_str::<EventId>(r#""k3m9x2a""#).unwrap(),
        EventId::fixed("k3m9x2a")
    );
}

#[test]
fn deserialize_rejects_an_empty_string() {
    let error = from_str::<EventId>(r#""""#).expect_err("empty event ID must be rejected");

    assert!(
        error.to_string().contains("Event ID must not be empty"),
        "unexpected message: {error}"
    );
}

#[test]
fn deserialize_rejects_a_non_string() {
    for json in ["null", "7", "true", "[]", "{}"] {
        assert!(from_str::<EventId>(json).is_err(), "accepted {json}");
    }
}

#[test]
fn a_hand_edited_id_outside_the_generated_format_round_trips() {
    // The lowercase-alphanumeric format is a generation convention, not a
    // parsing constraint, so a hand-edited file keeps whatever it holds.
    let raw = r#""Hand-edited ID: 42!""#;
    let id = from_str::<EventId>(raw).unwrap();

    assert_eq!(id, EventId::fixed("Hand-edited ID: 42!"));
    assert_eq!(to_string(&id).unwrap(), raw);
}

#[test]
fn an_id_holding_json_significant_characters_round_trips() {
    let raw = r#""quote\" backslash\\ brace}""#;
    let id = from_str::<EventId>(raw).unwrap();

    assert_eq!(to_string(&id).unwrap(), raw);
}

#[test]
fn a_non_ascii_id_round_trips() {
    let id = from_str::<EventId>(r#""é☃""#).unwrap();

    assert_eq!(id, EventId::fixed("\u{00e9}\u{2603}"));
    assert_eq!(to_string(&id).unwrap(), r#""é☃""#);
}

#[test]
fn a_whitespace_only_id_round_trips() {
    // Non-empty is the only rule, and whitespace satisfies it.
    let raw = r#"" \t\n ""#;
    let id = from_str::<EventId>(raw).unwrap();

    assert_eq!(id, EventId::fixed(" \t\n "));
    assert_eq!(to_string(&id).unwrap(), raw);
}

#[test]
fn display_writes_the_id_alone() {
    assert_eq!(EventId::fixed("k3m9x2a").to_string(), "k3m9x2a");
}

#[test]
fn debug_names_the_type() {
    assert_eq!(
        format!("{:?}", EventId::fixed("k3m9x2a")),
        r#"EventId("k3m9x2a")"#
    );
}

#[test]
fn a_generated_id_has_the_documented_length() {
    assert_eq!(EventId::random().to_string().len(), GENERATED_LEN);
}

#[test]
fn a_generated_id_uses_lowercase_base36() {
    // A thousand draws is enough to catch a biased or mis-masked byte leaking
    // a character outside the alphabet.
    for _ in 0..1_000 {
        let id = EventId::random().to_string();
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
            "generated ID outside the alphabet: {id}"
        );
    }
}

#[test]
fn two_draws_produce_distinct_ids() {
    assert_ne!(EventId::random(), EventId::random());
}

#[test]
#[should_panic(expected = "event ID must not be empty")]
fn a_fixed_id_must_not_be_empty() {
    // `EventId::fixed` is a fixture constructor, so it panics rather than
    // returning a `Result`: an empty literal in a test is a mistake in the
    // test.
    let _id = EventId::fixed("");
}

#[test]
fn claim_takes_an_id_the_set_does_not_hold() {
    let mut ids = EventIds::default();

    assert_eq!(ids.claim(EventId::fixed("inherited")), {
        EventId::fixed("inherited")
    });
}

#[test]
fn claim_replaces_an_id_the_set_already_holds() {
    let mut ids: EventIds = std::iter::once(EventId::fixed("taken")).collect();

    let claimed = ids.claim(EventId::fixed("taken"));

    assert_ne!(claimed, EventId::fixed("taken"));
    assert_eq!(claimed.to_string().len(), GENERATED_LEN);
}

#[test]
fn claim_replaces_an_id_it_handed_out_itself() {
    // An ID is retired with its entry, so a second claim of the same value
    // does not get it back. This is what makes a reference to a removed entry
    // fail rather than rebind.
    let mut ids = EventIds::default();
    ids.claim(EventId::fixed("once"));

    assert_ne!(ids.claim(EventId::fixed("once")), EventId::fixed("once"));
}

#[test]
fn a_draw_retries_past_an_id_the_set_already_holds() {
    let mut ids: EventIds = [EventId::fixed("taken01"), EventId::fixed("taken02")]
        .into_iter()
        .collect();
    let mut candidates = ["taken01", "taken02", "fresh01"]
        .map(EventId::fixed)
        .into_iter();

    let drawn = ids.draw(|| candidates.next().expect("scripted candidate"));

    assert_eq!(drawn, EventId::fixed("fresh01"));
    assert!(candidates.next().is_none(), "stopped before the third draw");
}

#[test]
fn a_draw_retries_past_an_id_the_set_handed_out_earlier() {
    let mut ids = EventIds::default();
    let mut candidates = ["first01", "first01", "second1"]
        .map(EventId::fixed)
        .into_iter();

    let first = ids.draw(|| candidates.next().expect("scripted candidate"));
    let second = ids.draw(|| candidates.next().expect("scripted candidate"));

    assert_eq!(first, EventId::fixed("first01"));
    assert_eq!(second, EventId::fixed("second1"));
}

#[test]
fn a_thousand_draws_produce_distinct_ids() {
    // The weak half of the pair: a thousand draws from a 36^7 space are almost
    // certainly distinct whether or not `fresh` checks the set, so what this
    // pins is that `EventId::random` varies at all — a generator returning one
    // value forever satisfies every other test in this file.
    // `a_draw_retries_past_an_id_the_set_handed_out_earlier` pins the retry
    // itself, deterministically.
    let mut ids = EventIds::default();
    let drawn: Vec<_> = (0..1_000).map(|_| ids.fresh()).collect();

    let unique: std::collections::HashSet<_> = drawn.iter().collect();
    assert_eq!(unique.len(), drawn.len());
}
