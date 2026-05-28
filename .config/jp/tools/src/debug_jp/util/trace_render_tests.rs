use std::time::Duration;

use serde_json::{Map, Value, json};

use super::*;
use crate::debug_jp::util::trace_parse::{Level, TraceEvent};

fn fixture_launch() -> LaunchResult {
    LaunchResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        wall_duration: Duration::from_millis(1234),
    }
}

fn fixture_paths<'a>() -> OutputPaths<'a> {
    OutputPaths {
        trace: "tmp/profiling/trace-x.jsonl",
        stdout: "tmp/profiling/trace-x-stdout.txt",
        stderr: "tmp/profiling/trace-x-stderr.txt",
    }
}

fn event(level: Level, target: &str, message: &str) -> TraceEvent {
    TraceEvent {
        timestamp: "2026-05-25T21:44:04.572200Z".into(),
        level,
        target: target.into(),
        message: message.into(),
        fields: Map::new(),
        spans: Vec::new(),
    }
}

#[test]
fn format_time_keeps_ms_only() {
    assert_eq!(format_time("2026-05-25T21:44:04.572200Z"), "21:44:04.572");
    assert_eq!(format_time("2026-05-25T21:44:04Z"), "21:44:04");
    // Fallback: pass through if not RFC3339-ish.
    assert_eq!(format_time("bogus"), "bogus");
}

#[test]
fn format_value_unquoted_simple_token() {
    assert_eq!(format_value(&json!("simple")), "simple");
    assert_eq!(format_value(&json!(42)), "42");
    assert_eq!(format_value(&json!(true)), "true");
    assert_eq!(format_value(&Value::Null), "null");
}

#[test]
fn format_value_quotes_strings_with_spaces() {
    assert_eq!(format_value(&json!("two words")), "\"two words\"");
}

#[test]
fn format_value_escapes_inner_quotes() {
    assert_eq!(
        format_value(&json!("she said \"hi\"")),
        "\"she said \\\"hi\\\"\""
    );
}

#[test]
fn format_value_does_not_truncate_long_values() {
    // Long values are rendered in full. The assistant reading the report can
    // parse long lines; truncation was hiding the differentiating tail of
    // path-shaped values.
    let long = "x".repeat(500);
    let formatted = format_value(&json!(long));
    assert_eq!(formatted.chars().count(), 500);
    assert!(!formatted.contains('…'));
}

#[test]
fn format_value_serializes_nested_objects_as_json() {
    let v = json!({"a": 1, "b": [2, 3]});
    let formatted = format_value(&v);
    // Nested values serialize to a JSON string, which is then quoted as a
    // field value; the inner double-quotes are backslash-escaped.
    assert!(
        formatted.contains("\\\"a\\\":1"),
        "expected escaped `\\\"a\\\":1` in {formatted}"
    );
    assert!(formatted.starts_with('"') && formatted.ends_with('"'));
}

#[test]
fn render_includes_headline_and_summary() {
    let report = render(
        &[event(Level::Info, "jp_cli", "starting")],
        1,
        &fixture_launch(),
        &["c".into(), "fork".into()],
        fixture_paths(),
    );
    assert!(report.contains("# jp debug · trace"));
    assert!(report.contains("`jp c fork`"));
    assert!(report.contains("**Wall clock:** 1.23 s"));
    assert!(report.contains("**Status:** success"));
    assert!(report.contains("**Events:** 1"));
}

#[test]
fn render_shows_filtered_count_when_subset() {
    let report = render(
        &[event(Level::Info, "jp_cli", "x")],
        50,
        &fixture_launch(),
        &["c".into()],
        fixture_paths(),
    );
    assert!(report.contains("**Events:** 1 shown / 50 total"));
}

#[test]
fn render_event_line_has_time_level_target_message() {
    let report = render(
        &[event(Level::Warn, "jp_config", "something")],
        1,
        &fixture_launch(),
        &["c".into()],
        fixture_paths(),
    );
    assert!(
        report.contains("21:44:04.572"),
        "expected time; got:\n{report}"
    );
    assert!(
        report.contains("WARN "),
        "expected padded WARN level; got:\n{report}"
    );
    assert!(report.contains("jp_config"));
    assert!(report.contains("something"));
}

#[test]
fn render_includes_field_pairs_in_order() {
    let mut e = event(Level::Info, "jp_cli", "loaded");
    e.fields.insert("path".into(), json!("/x/y"));
    e.fields.insert("size".into(), json!(42));

    let report = render(&[e], 1, &fixture_launch(), &["c".into()], fixture_paths());
    let path_pos = report.find("path=").expect("path field missing");
    let size_pos = report.find("size=").expect("size field missing");
    assert!(path_pos < size_pos);
    assert!(report.contains("path=/x/y"));
    assert!(report.contains("size=42"));
}

#[test]
fn render_appends_span_stack() {
    let mut e = event(Level::Debug, "jp_config", "merging");
    e.spans = vec!["load_partial_config".into(), "parse_into_layers".into()];

    let report = render(&[e], 1, &fixture_launch(), &["c".into()], fixture_paths());
    assert!(report.contains("[load_partial_config > parse_into_layers]"));
}

#[test]
fn render_collapses_consecutive_identical_events() {
    // Three byte-identical events (only `timestamp` differs). They should
    // collapse into one line with `× 3`.
    let make_event = |ts: &str| TraceEvent {
        timestamp: ts.into(),
        level: Level::Info,
        target: "jp_config".into(),
        message: "tick".into(),
        fields: Map::new(),
        spans: Vec::new(),
    };
    let events = vec![
        make_event("2026-05-25T21:44:04.001Z"),
        make_event("2026-05-25T21:44:04.002Z"),
        make_event("2026-05-25T21:44:04.003Z"),
    ];

    let report = render(&events, 3, &fixture_launch(), &["c".into()], fixture_paths());
    assert!(report.contains("× 3"), "expected `× 3` in:\n{report}");
    // Only one event line in the fenced block — count occurrences of the
    // message "tick".
    let occurrences = report.matches("tick").count();
    assert_eq!(occurrences, 1, "expected exactly one tick line; got:\n{report}");
}

#[test]
fn render_does_not_collapse_events_with_different_fields() {
    // Same target+message, different `path=`. These look identical at a
    // glance but represent different work (different files loaded). They
    // must stay distinct.
    let mut a = event(Level::Info, "jp_config::util", "Found configuration file.");
    a.fields.insert("path".into(), json!("/a.toml"));
    let mut b = event(Level::Info, "jp_config::util", "Found configuration file.");
    b.fields.insert("path".into(), json!("/b.toml"));
    let mut c = event(Level::Info, "jp_config::util", "Found configuration file.");
    c.fields.insert("path".into(), json!("/c.toml"));

    let report = render(
        &[a, b, c],
        3,
        &fixture_launch(),
        &["c".into()],
        fixture_paths(),
    );
    assert!(!report.contains("× "), "should not collapse; got:\n{report}");
    assert!(report.contains("path=/a.toml"));
    assert!(report.contains("path=/b.toml"));
    assert!(report.contains("path=/c.toml"));
}

#[test]
fn render_collapses_only_consecutive_runs() {
    // A, A, B, A, A — the two A-runs collapse separately; B in between is
    // preserved so the chronology is honest.
    let a = || event(Level::Info, "jp_cli", "a");
    let b = || event(Level::Info, "jp_cli", "b");

    let report = render(
        &[a(), a(), b(), a(), a()],
        5,
        &fixture_launch(),
        &["c".into()],
        fixture_paths(),
    );
    // Two collapsed runs (each "× 2"), one standalone b.
    let two_count = report.matches("× 2").count();
    assert_eq!(two_count, 2);
    assert!(report.contains("  a"));
    assert!(report.contains("  b"));
}

#[test]
fn render_truncates_at_soft_cap() {
    // Distinct messages so collapsing doesn't kick in.
    let events: Vec<TraceEvent> = (0..SOFT_CAP + 50)
        .map(|i| event(Level::Info, "x", &format!("event {i}")))
        .collect();
    let total = events.len();
    let report = render(&events, total, &fixture_launch(), &["c".into()], fixture_paths());
    assert!(report.contains("event 0"));
    assert!(!report.contains("event 405"));
    assert!(report.contains("more events omitted"));
}

#[test]
fn render_empty_subset_says_so() {
    let report = render(&[], 100, &fixture_launch(), &["c".into()], fixture_paths());
    assert!(report.contains("No events match the current filter"));
}

#[test]
fn render_includes_stdout_when_non_empty() {
    let mut launch = fixture_launch();
    launch.stdout = "hello from jp\n".into();

    let report = render(&[], 0, &launch, &["query".into()], fixture_paths());
    assert!(report.contains("## stdout"));
    assert!(report.contains("hello from jp"));
}

#[test]
fn render_omits_stdout_section_when_empty() {
    let report = render(&[], 0, &fixture_launch(), &["c".into()], fixture_paths());
    assert!(!report.contains("## stdout"));
}

#[test]
fn render_strips_trace_path_marker_from_stderr() {
    // The marker line jp emits to advertise the trace path should not
    // appear inside the rendered stderr block — it's already in the footer.
    let mut launch = fixture_launch();
    launch.stderr = "some real error\nFull trace log written to: /tmp/x\n".into();

    let report = render(&[], 0, &launch, &["c".into()], fixture_paths());
    assert!(report.contains("## stderr"));
    assert!(report.contains("some real error"));
    // The marker line appears in the footer reference, not inside the
    // stderr fenced block. We verify by checking that the stderr block
    // closes before the marker text could possibly appear.
    let stderr_start = report.find("## stderr").unwrap();
    let footer_start = report.find("**Files:**").unwrap();
    let stderr_section = &report[stderr_start..footer_start];
    assert!(!stderr_section.contains("Full trace log written to"));
}

#[test]
fn render_footer_lists_all_three_sidecar_files() {
    let report = render(&[], 0, &fixture_launch(), &["c".into()], fixture_paths());
    assert!(report.contains("- Trace: `tmp/profiling/trace-x.jsonl`"));
    assert!(report.contains("- Stdout: `tmp/profiling/trace-x-stdout.txt`"));
    assert!(report.contains("- Stderr: `tmp/profiling/trace-x-stderr.txt`"));
}
