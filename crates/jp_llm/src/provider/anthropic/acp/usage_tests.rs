use std::time::{Duration, Instant};

use async_anthropic::types::CreateMessagesResponse;
use serde_json::json;

use super::*;

fn cached(read: u64, write: u64) -> Usage {
    serde_json::from_value(json!({
        "input_tokens": 2,
        "output_tokens": 0,
        "cache_read_input_tokens": read,
        "cache_creation_input_tokens": write,
    }))
    .unwrap()
}

// Live values from one turn: each request read exactly what the request before
// it read and wrote.
#[test]
fn a_turn_that_reuses_its_cache_raises_no_notice() {
    let start = Instant::now();
    let mut continuity = CacheContinuity::default();

    let notices = [(71_368, 300_410), (371_778, 1_155), (372_933, 295)]
        .into_iter()
        .enumerate()
        .map(|(index, (read, write))| {
            continuity.check(
                "claude-opus-5-5",
                &cached(read, write),
                start + Duration::from_secs(10 * index as u64),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(notices, [None, None, None]);
}

#[test]
fn a_request_that_reads_less_than_the_previous_one_cached_raises_a_notice() {
    let start = Instant::now();
    let mut continuity = CacheContinuity::default();
    assert_eq!(
        continuity.check("claude-opus-5-5", &cached(8_002, 1_293), start),
        None
    );

    let notice = continuity.check(
        "claude-opus-5-5",
        &cached(7_131, 2_164),
        start + Duration::from_secs(10),
    );

    assert_eq!(
        notice.as_deref(),
        Some(
            "prompt cache not reused within the turn: read 7131 cached tokens, the previous \
             request cached 9295; please report this to JP"
        )
    );
}

// A model change starts a new cache, so a smaller read is expected.
#[test]
fn a_model_change_raises_no_notice() {
    let start = Instant::now();
    let mut continuity = CacheContinuity::default();
    continuity.check("claude-opus-5-5", &cached(8_002, 1_293), start);

    let notice = continuity.check(
        "claude-sonnet-5-5",
        &cached(0, 9_300),
        start + Duration::from_secs(10),
    );

    assert_eq!(notice, None);
}

// An approval left open past the cache's lifetime expires the entry, so a gap
// that long is no evidence of a bug.
#[test]
fn a_gap_past_the_shortest_cache_lifetime_raises_no_notice() {
    let start = Instant::now();
    let mut continuity = CacheContinuity::default();
    continuity.check("claude-opus-5-5", &cached(8_002, 1_293), start);

    let notice = continuity.check(
        "claude-opus-5-5",
        &cached(7_131, 2_164),
        start + Duration::from_mins(5),
    );

    assert_eq!(notice, None);
}

// After a skipped comparison, the next pair is still checked.
#[test]
fn a_skipped_comparison_still_sets_the_baseline() {
    let start = Instant::now();
    let mut continuity = CacheContinuity::default();
    continuity.check("claude-opus-5-5", &cached(8_002, 1_293), start);
    continuity.check(
        "claude-opus-5-5",
        &cached(7_131, 2_164),
        start + Duration::from_mins(5),
    );

    let notice = continuity.check(
        "claude-opus-5-5",
        &cached(100, 0),
        start + Duration::from_secs(5 * 60 + 10),
    );

    assert_eq!(
        notice.as_deref(),
        Some(
            "prompt cache not reused within the turn: read 100 cached tokens, the previous \
             request cached 9295; please report this to JP"
        )
    );
}

// Caching off reports no reads and no writes; there is nothing to compare.
#[test]
fn requests_without_cache_counts_raise_no_notice() {
    let start = Instant::now();
    let mut continuity = CacheContinuity::default();
    let uncounted: Usage =
        serde_json::from_value(json!({"input_tokens": 9000, "output_tokens": 0})).unwrap();
    continuity.check("claude-opus-5-5", &cached(8_002, 1_293), start);

    assert_eq!(
        continuity.check(
            "claude-opus-5-5",
            &uncounted,
            start + Duration::from_secs(10)
        ),
        None
    );
}

#[test]
fn repeated_message_ids_replace_snapshots_instead_of_adding_tokens() {
    let first: CreateMessagesResponse = serde_json::from_value(json!({"id":"msg-fixed","model":"claude-opus-5","usage":{"input_tokens":12,"output_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":200}})).unwrap();
    let later: CreateMessagesResponse = serde_json::from_value(
        json!({"id":"msg-fixed","model":"claude-opus-5","usage":{"output_tokens":7}}),
    )
    .unwrap();
    let mut ledger = UsageLedger::default();
    ledger.observe(&first);
    ledger.observe(&later);
    assert_eq!(
        ledger.snapshot("session-fixed"),
        json!({
            "native_session_id":"session-fixed",
            "requests":{"msg-fixed":{"model":"claude-opus-5","input_tokens":12,"output_tokens":7,"cache_creation_input_tokens":100,"cache_read_input_tokens":200}}
        })
    );
}

#[test]
fn runtime_totals_are_separate_from_main_request_usage() {
    let message: CreateMessagesResponse = serde_json::from_value(json!({"id":"msg-main","model":"claude-opus-5","usage":{"input_tokens":2,"output_tokens":4,"cache_creation_input_tokens":0,"cache_read_input_tokens":500}})).unwrap();
    let mut ledger = UsageLedger::default();
    ledger.observe(&message);
    ledger.set_runtime(serde_json::from_value(json!({
        "usage":{"input_tokens":9,"output_tokens":10,"cache_creation_input_tokens":0,"cache_read_input_tokens":500},
        "model_usage":{"claude-opus-5":{"inputTokens":2,"outputTokens":4},"claude-haiku-4-5":{"inputTokens":7,"outputTokens":6}},
        "estimated_cost_usd":0.02
    })).unwrap());
    let snapshot = ledger.snapshot("session-fixed");
    assert_eq!(snapshot["requests"]["msg-main"]["input_tokens"], 2);
    assert_eq!(snapshot["runtime"]["usage"]["input_tokens"], 9);
    assert_eq!(
        snapshot["runtime"]["model_usage"]["claude-haiku-4-5"]["outputTokens"],
        6
    );
    assert_eq!(snapshot["runtime"]["estimated_cost_usd"], 0.02);
}

#[test]
fn missing_usage_is_not_reported_as_zero() {
    let message: CreateMessagesResponse =
        serde_json::from_value(json!({"id":"msg-fixed","model":"claude-opus-5"})).unwrap();
    let mut ledger = UsageLedger::default();
    ledger.observe(&message);
    assert_eq!(
        ledger.snapshot("session-fixed"),
        json!({"native_session_id":"session-fixed","requests":{}})
    );
}
