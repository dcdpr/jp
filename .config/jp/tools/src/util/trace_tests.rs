use super::*;

#[test]
fn level_ordering_allows_threshold_filtering() {
    assert!(Level::Info >= Level::Info);
    assert!(Level::Warn >= Level::Info);
    assert!(Level::Error >= Level::Info);
    assert!(Level::Debug < Level::Info);
    assert!(Level::Trace < Level::Info);
}

#[test]
fn level_parses_case_insensitively() {
    assert_eq!(Level::parse("info"), Some(Level::Info));
    assert_eq!(Level::parse("INFO"), Some(Level::Info));
    assert_eq!(Level::parse("Warn"), Some(Level::Warn));
    assert_eq!(Level::parse("warning"), Some(Level::Warn));
    assert_eq!(Level::parse("ERROR"), Some(Level::Error));
    assert_eq!(Level::parse("nope"), None);
}

#[test]
fn parse_extracts_message_out_of_fields() {
    let line = r#"{"timestamp":"2026-05-25T21:44:04.572Z","level":"INFO","fields":{"message":"hello","path":"/tmp/x"},"target":"jp_cli"}"#;
    let events = parse_lines(line);
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.timestamp, "2026-05-25T21:44:04.572Z");
    assert_eq!(event.level, Level::Info);
    assert_eq!(event.target, "jp_cli");
    assert_eq!(event.message, "hello");
    // The `message` key is removed from fields; only `path` remains.
    assert_eq!(event.fields.len(), 1);
    assert_eq!(
        event.fields.get("path").and_then(Value::as_str),
        Some("/tmp/x")
    );
}

#[test]
fn parse_extracts_span_stack() {
    let line = r#"{"timestamp":"2026-05-25T21:44:04.572Z","level":"DEBUG","fields":{"message":"x"},"target":"jp_cli","spans":[{"name":"outer"},{"name":"inner"}]}"#;
    let events = parse_lines(line);
    assert_eq!(events[0].spans, vec![
        "outer".to_owned(),
        "inner".to_owned()
    ]);
}

#[test]
fn parse_skips_blank_lines_and_malformed_lines() {
    let lines = format!(
        "{}\n\n{}\nnot json at all\n{}",
        r#"{"timestamp":"t1","level":"INFO","fields":{"message":"a"},"target":"x"}"#,
        r#"{"timestamp":"t2","level":"INFO","fields":{"message":"b"},"target":"x"}"#,
        r#"{"timestamp":"t3","level":"INFO","fields":{"message":"c"},"target":"x"}"#,
    );
    let events = parse_lines(&lines);
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].message, "a");
    assert_eq!(events[1].message, "b");
    assert_eq!(events[2].message, "c");
}

#[test]
fn parse_tolerates_missing_optional_fields() {
    // No `fields`, no `spans`. Still a valid event.
    let line = r#"{"timestamp":"t","level":"WARN","target":"x"}"#;
    let events = parse_lines(line);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "");
    assert!(events[0].fields.is_empty());
    assert!(events[0].spans.is_empty());
}

#[test]
fn parse_drops_lines_with_unknown_level() {
    // `WTF` isn't a valid level. The line is skipped, not promoted to an error.
    let line = r#"{"timestamp":"t","level":"WTF","fields":{"message":"x"},"target":"x"}"#;
    let events = parse_lines(line);
    assert!(events.is_empty());
}

#[test]
fn extract_trace_path_reads_text_marker_line() {
    let stderr = "some noise\nFull trace log written to: /tmp/x.jsonl\n";
    assert_eq!(extract_trace_path(stderr), Some("/tmp/x.jsonl".to_owned()));
}

#[test]
fn extract_trace_path_reads_json_marker_line() {
    // jp emits this shape instead of the text marker when `--format` is
    // json or json-pretty.
    let stderr = "some noise\n{\"trace_log\":\"/tmp/x.jsonl\"}\n";
    assert_eq!(extract_trace_path(stderr), Some("/tmp/x.jsonl".to_owned()));
}

#[test]
fn extract_trace_path_returns_none_without_a_marker() {
    assert_eq!(extract_trace_path("nothing relevant here\n"), None);
}

#[test]
fn is_trace_path_marker_line_matches_both_formats() {
    assert!(is_trace_path_marker_line(
        "Full trace log written to: /tmp/x.jsonl"
    ));
    assert!(is_trace_path_marker_line(r#"{"trace_log":"/tmp/x.jsonl"}"#));
    assert!(!is_trace_path_marker_line("some real error"));
}

#[test]
fn parse_preserves_field_insertion_order() {
    // serde_json's `preserve_order` feature is enabled at the workspace level.
    // Field order in the JSON should be preserved through the parse step.
    let line = r#"{"timestamp":"t","level":"INFO","fields":{"message":"x","z_last":1,"a_first":2,"m_middle":3},"target":"x"}"#;
    let events = parse_lines(line);
    let keys: Vec<&str> = events[0].fields.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["z_last", "a_first", "m_middle"]);
}
