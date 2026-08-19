use super::{render, summarize};
use crate::util::trace::parse_lines;

/// One line exactly as the app writes it.
///
/// Pinned here and in `TraceTests.swift`, character for character.
/// Nothing else checks that the writer and the reader agree on the format: if
/// one of these two strings is edited alone, the other test is what says so.
const APP_LINE: &str = r#"{"timestamp":"2026-08-02T11:04:12.418293Z","level":"INFO","target":"JP.Transcript","fields":{"message":"transcript.render","duration_ms":84.219,"event_count":847,"footprint_mb":412},"spans":[{"name":"conversation.select"}]}"#;

#[test]
fn summarizes_a_line_the_app_wrote() {
    let summary = summarize(&parse_lines(APP_LINE));

    assert_eq!(summary.intervals, 1);
    assert_eq!(
        summary.slowest,
        Some(("transcript.render".to_owned(), 84.219))
    );
    assert_eq!(summary.footprint_mb, Some(412));
}

/// The block a snapshot prints beside its console delta.
#[test]
fn renders_the_slowest_interval_and_the_footprint_change() {
    let summary = summarize(&parse_lines(APP_LINE));

    assert_eq!(
        render(&summary, Some(374)).as_deref(),
        Some(
            "Trace, since the last call: 1 span. Slowest `transcript.render` 84 ms.\nFootprint \
             412 MB (+38 MB)."
        )
    );
}

/// The first snapshot of a run has nothing to compare against, and a `(+412
/// MB)` there would read as a spike rather than as the app's whole footprint.
#[test]
fn omits_the_change_when_nothing_was_reported_before() {
    let summary = summarize(&parse_lines(APP_LINE));

    assert_eq!(
        render(&summary, None).as_deref(),
        Some(
            "Trace, since the last call: 1 span. Slowest `transcript.render` 84 ms.\nFootprint \
             412 MB."
        )
    );
}

#[test]
fn counts_every_interval_and_keeps_the_longest() {
    let lines = [
        r#"{"timestamp":"2026-08-02T11:04:12.000000Z","level":"INFO","target":"JP.View","fields":{"message":"WorkspaceWindow.body","duration_ms":0.42,"footprint_mb":400}}"#,
        r#"{"timestamp":"2026-08-02T11:04:12.100000Z","level":"INFO","target":"JP.View","fields":{"message":"ConversationHistoryView.body","duration_ms":12.5,"footprint_mb":404}}"#,
        r#"{"timestamp":"2026-08-02T11:04:12.200000Z","level":"INFO","target":"JP.View","fields":{"message":"WorkspaceWindow.body","duration_ms":0.31,"footprint_mb":405}}"#,
    ]
    .join("\n");

    let summary = summarize(&parse_lines(&lines));

    assert_eq!(summary.intervals, 3);
    assert_eq!(
        render(&summary, Some(405)).as_deref(),
        Some(
            "Trace, since the last call: 3 spans. Slowest `ConversationHistoryView.body` 13 \
             ms.\nFootprint 405 MB."
        )
    );
}

/// The reference pair the app writes at startup times nothing, so a snapshot
/// that only saw it should not claim an interval.
#[test]
fn reports_an_origin_event_as_no_intervals() {
    let origin = r#"{"timestamp":"2026-08-02T11:04:10.000000Z","level":"INFO","target":"JP.Trace","fields":{"message":"trace.origin","mach_absolute_time":42000,"unix_time_ns":1785668650000000000,"timebase_numer":125,"timebase_denom":3}}"#;

    let summary = summarize(&parse_lines(origin));

    assert_eq!(summary.intervals, 0);
    assert_eq!(
        render(&summary, None).as_deref(),
        Some("Trace, since the last call: no intervals.")
    );
}

/// An app that traced nothing since the last call gets no block at all, rather
/// than a line saying so: the snapshot already carries three other sections.
#[test]
fn renders_nothing_when_the_delta_is_empty() {
    assert_eq!(render(&summarize(&parse_lines("")), Some(400)), None);
}
