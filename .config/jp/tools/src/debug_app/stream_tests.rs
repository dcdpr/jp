use std::fs;

use camino::Utf8Path;
use serde_json::{Value, json};

use super::{count, load, millis_label, tally};
use crate::debug_app::{capture::profiles_dir, session::state_dir};

/// One interval, in the shape `Trace.swift` writes.
fn line(
    timestamp: &str,
    target: &str,
    message: &str,
    duration_ms: f64,
    footprint_mb: u64,
) -> Value {
    json!({
        "timestamp": timestamp,
        "level": "INFO",
        "target": target,
        "fields": {
            "message": message,
            "duration_ms": duration_ms,
            "footprint_mb": footprint_mb,
        },
    })
}

/// The same, nested inside an enclosing interval.
fn nested(timestamp: &str, target: &str, message: &str, duration_ms: f64, span: &str) -> Value {
    json!({
        "timestamp": timestamp,
        "level": "INFO",
        "target": target,
        "fields": { "message": message, "duration_ms": duration_ms },
        "spans": [{ "name": span }],
    })
}

fn stream(lines: &[Value]) -> String {
    format!(
        "{}\n",
        lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn write_live(dir: &Utf8Path, lines: &[Value]) {
    let state = state_dir(dir);
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("trace.jsonl"), stream(lines)).unwrap();
}

fn write_archived(dir: &Utf8Path, id: &str, lines: &[Value]) {
    fs::create_dir_all(profiles_dir(dir)).unwrap();
    fs::write(profiles_dir(dir).join(format!("{id}.jsonl")), stream(lines)).unwrap();
}

/// One line exactly as `Trace.swift` writes it.
///
/// A raw literal rather than a builder, so the parser is pinned against the
/// format itself rather than against something that agrees with it by
/// construction.
const EXACT_LINE: &str = r#"{"timestamp":"2026-08-03T10:00:00.500000Z","level":"INFO","target":"JP.App","fields":{"message":"conversation.select","duration_ms":12.5,"footprint_mb":184}}"#;

#[test]
fn an_interval_carries_its_name_target_duration_and_moment() {
    let dir = camino_tempfile::tempdir().unwrap();
    let state = state_dir(dir.path());
    fs::create_dir_all(&state).unwrap();
    fs::write(state.join("trace.jsonl"), format!("{EXACT_LINE}\n")).unwrap();

    let intervals = load(dir.path());

    assert_eq!(intervals.len(), 1);
    assert_eq!(intervals[0].name, "conversation.select");
    assert_eq!(intervals[0].target, "JP.App");
    assert_eq!(format!("{:.1}", intervals[0].duration_ms), "12.5");
    assert_eq!(intervals[0].at_ms, 1_785_751_200_500);
    assert_eq!(intervals[0].footprint_mb, Some(184));
    assert!(intervals[0].is_top_level());
}

/// The moment work began is what attributes it, and the app records only the
/// moment it ended.
/// A 49ms selection that ends 9ms after the harness closed its step began well
/// inside it.
#[test]
fn an_interval_knows_when_it_began_as_well_as_when_it_ended() {
    let dir = camino_tempfile::tempdir().unwrap();
    write_live(dir.path(), &[line(
        "2026-08-03T12:12:39.208887Z",
        "JP.App",
        "conversation.select",
        48.932,
        42,
    )]);

    let intervals = load(dir.path());

    assert_eq!(intervals[0].at_ms, 1_785_759_159_208);
    assert_eq!(intervals[0].started_ms, 1_785_759_159_160);
}

/// Ordered by start, so an enclosing interval reads ahead of the work inside it
/// rather than after it: a parent ends last but begins first.
#[test]
fn intervals_read_in_the_order_the_work_began() {
    let dir = camino_tempfile::tempdir().unwrap();
    write_live(dir.path(), &[
        nested(
            "2026-08-03T10:00:00.030000Z",
            "JP.FFI",
            "storage.read",
            10.0,
            "conversation.select",
        ),
        line(
            "2026-08-03T10:00:00.100000Z",
            "JP.App",
            "conversation.select",
            100.0,
            180,
        ),
    ]);

    let names: Vec<String> = load(dir.path())
        .iter()
        .map(|interval| interval.name.clone())
        .collect();

    assert_eq!(names, vec!["conversation.select", "storage.read"]);
}

/// `trace.origin` carries the pair that lines this timeline up with a mach one
/// and times nothing, so it is not an interval.
#[test]
fn an_event_with_no_duration_is_not_an_interval() {
    let dir = camino_tempfile::tempdir().unwrap();
    write_live(dir.path(), &[
        json!({
            "timestamp": "2026-08-03T10:00:00.000000Z",
            "level": "INFO",
            "target": "JP.Trace",
            "fields": { "message": "trace.origin", "mach_absolute_time": 42 },
        }),
        line(
            "2026-08-03T10:00:01.000000Z",
            "JP.App",
            "app.launch",
            900.0,
            120,
        ),
    ]);

    let intervals = load(dir.path());

    assert_eq!(intervals.len(), 1);
    assert_eq!(intervals[0].name, "app.launch");
}

/// Archived streams and the live one are one timeline, which is what makes
/// comparing two runs possible at all.
#[test]
fn archived_and_live_streams_read_as_one_sorted_sequence() {
    let dir = camino_tempfile::tempdir().unwrap();
    write_archived(dir.path(), "trace-1000", &[line(
        "2026-08-03T09:00:00.000000Z",
        "JP.App",
        "earlier.run",
        1.0,
        100,
    )]);
    write_live(dir.path(), &[line(
        "2026-08-03T10:00:00.000000Z",
        "JP.App",
        "this.run",
        2.0,
        110,
    )]);

    let names: Vec<String> = load(dir.path())
        .iter()
        .map(|interval| interval.name.clone())
        .collect();

    assert_eq!(names, vec!["earlier.run", "this.run"]);
}

#[test]
fn counts_lead_and_only_outermost_intervals_add_to_the_traced_total() {
    let dir = camino_tempfile::tempdir().unwrap();
    write_live(dir.path(), &[
        line(
            "2026-08-03T10:00:00.000000Z",
            "JP.App",
            "conversation.select",
            100.0,
            180,
        ),
        nested(
            "2026-08-03T10:00:00.010000Z",
            "JP.FFI",
            "storage.read",
            30.0,
            "conversation.select",
        ),
        nested(
            "2026-08-03T10:00:00.020000Z",
            "JP.FFI",
            "deserialize",
            20.0,
            "conversation.select",
        ),
        line(
            "2026-08-03T10:00:00.030000Z",
            "JP.App",
            "ConversationHistoryView.body",
            0.4,
            182,
        ),
    ]);

    let intervals = load(dir.path());
    let counts = count(&intervals.iter().collect::<Vec<_>>());

    assert_eq!(counts.intervals, 4);
    assert_eq!(counts.view_bodies, 1);
    assert_eq!(counts.ffi_calls, 2);
    assert_eq!(format!("{:.1}", counts.traced_ms), "100.4");
    assert_eq!(counts.footprint_mb, Some(182));
}

/// The footprint is the sample taken last, and a sample is taken when an
/// interval *ends*.
/// Since these are ordered by start, the enclosing interval comes first in the
/// slice and ends last, so taking whichever came last in iteration order would
/// report a nested interval's figure as the enclosing one's.
#[test]
fn the_footprint_is_the_last_sample_taken_rather_than_the_last_one_listed() {
    let dir = camino_tempfile::tempdir().unwrap();
    write_live(dir.path(), &[
        line(
            "2026-08-03T10:00:00.200000Z",
            "JP.App",
            "conversation.select",
            100.0,
            250,
        ),
        line(
            "2026-08-03T10:00:00.150000Z",
            "JP.App",
            "ConversationHistoryView.body",
            1.0,
            190,
        ),
    ]);

    let intervals = load(dir.path());

    // The selection begins first and ends last.
    assert_eq!(intervals[0].name, "conversation.select");
    assert_eq!(
        count(&intervals.iter().collect::<Vec<_>>()).footprint_mb,
        Some(250)
    );
}

#[test]
fn a_tally_orders_by_how_often_each_name_ran() {
    let dir = camino_tempfile::tempdir().unwrap();
    write_live(dir.path(), &[
        line(
            "2026-08-03T10:00:00.000000Z",
            "JP.App",
            "WorkspaceWindow.body",
            5.0,
            100,
        ),
        line(
            "2026-08-03T10:00:00.001000Z",
            "JP.App",
            "ConversationHistoryView.body",
            1.0,
            100,
        ),
        line(
            "2026-08-03T10:00:00.002000Z",
            "JP.App",
            "ConversationHistoryView.body",
            3.0,
            100,
        ),
    ]);

    let intervals = load(dir.path());
    let tallied = tally(&intervals.iter().collect::<Vec<_>>());

    assert_eq!(tallied.len(), 2);
    assert_eq!(tallied[0].0, "ConversationHistoryView.body");
    assert_eq!(tallied[0].1.count, 2);
    assert_eq!(format!("{:.1}", tallied[0].1.total_ms), "4.0");
    assert_eq!(format!("{:.1}", tallied[0].1.max_ms), "3.0");
    assert_eq!(format!("{:.1}", tallied[0].1.mean_ms()), "2.0");
    assert_eq!(tallied[1].0, "WorkspaceWindow.body");
}

#[test]
fn a_slot_with_no_stream_reads_as_empty() {
    let dir = camino_tempfile::tempdir().unwrap();

    assert_eq!(load(dir.path()), Vec::new());
    assert!(!super::is_present(dir.path()));
}

/// Sub-millisecond work is routine — a view body is tens of microseconds —
/// and rounding it all to `0 ms` would hide which of two bodies is expensive.
#[test]
fn durations_keep_a_decimal_only_where_one_carries_meaning() {
    assert_eq!(millis_label(0.42), "0.4 ms");
    assert_eq!(millis_label(9.96), "10.0 ms");
    assert_eq!(millis_label(1104.4), "1104 ms");
}
