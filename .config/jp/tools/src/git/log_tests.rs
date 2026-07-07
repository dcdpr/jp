use camino_tempfile::tempdir;

use super::*;
use crate::util::runner::MockProcessRunner;

fn sample_log_output() -> String {
    [
        "abc123full\0abc123\0Alice\x002024-06-01T10:00:00+00:00\0feat: add widget",
        "def456full\0def456\0Bob\x002024-05-31T09:00:00+00:00\0fix: correct typo",
    ]
    .join("\n")
}

#[test]
fn parses_log_entries() {
    let entries = parse_log_entries(&sample_log_output());
    assert_eq!(entries.len(), 2);

    assert_eq!(entries[0].short_hash, "abc123");
    assert_eq!(entries[0].author, "Alice");
    assert_eq!(entries[0].subject, "feat: add widget");

    assert_eq!(entries[1].short_hash, "def456");
    assert_eq!(entries[1].subject, "fix: correct typo");
}

#[test]
fn parses_empty_output() {
    let entries = parse_log_entries("");
    assert!(entries.is_empty());
}

#[test]
fn parses_malformed_line_skipped() {
    let entries = parse_log_entries("this is not a valid log line\n");
    assert!(entries.is_empty());
}

#[test]
fn basic_log() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success(sample_log_output());

    let content = git_log_impl(dir.path(), None, None, false, &[], 20, None, &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert!(content.contains("<git_log>"));
    assert!(content.contains("    short_hash: abc123"));
    assert!(content.contains("    subject: feat: add widget"));
    assert!(content.contains("    short_hash: def456"));
}

#[test]
fn log_with_query() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "log",
            &format!("--format={LOG_FORMAT}"),
            "-n",
            "20",
            "--fixed-strings",
            "--grep=widget",
        ])
        .returns_success(sample_log_output());

    let content = git_log_impl(
        dir.path(),
        Some("widget"),
        None,
        false,
        &[],
        20,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("abc123"));
}

#[test]
fn log_with_content() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "log",
            &format!("--format={LOG_FORMAT}"),
            "-n",
            "20",
            "-Sc == '/'",
        ])
        .returns_success(sample_log_output());

    let content = git_log_impl(
        dir.path(),
        None,
        Some("c == '/'"),
        false,
        &[],
        20,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("abc123"));
}

#[test]
fn log_with_content_regex() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "log",
            &format!("--format={LOG_FORMAT}"),
            "-n",
            "20",
            "-Gfn from_str\\(",
        ])
        .returns_success(sample_log_output());

    let content = git_log_impl(
        dir.path(),
        None,
        Some("fn from_str\\("),
        true,
        &[],
        20,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("abc123"));
}

#[test]
fn log_content_regex_without_content_errors() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::never_called();

    let outcome = git_log_impl(dir.path(), None, None, true, &[], 20, None, &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none(), "expected error outcome");
}

#[test]
fn log_with_paths() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "log",
            &format!("--format={LOG_FORMAT}"),
            "-n",
            "10",
            "--",
            "src/main.rs",
        ])
        .returns_success(sample_log_output());

    let content = git_log_impl(
        dir.path(),
        None,
        None,
        false,
        &["src/main.rs"],
        10,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("abc123"));
}

#[test]
fn log_with_since() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&[
            "log",
            &format!("--format={LOG_FORMAT}"),
            "-n",
            "20",
            "--since=2 weeks ago",
        ])
        .returns_success(sample_log_output());

    let content = git_log_impl(
        dir.path(),
        None,
        None,
        false,
        &[],
        20,
        Some("2 weeks ago"),
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.contains("abc123"));
}

#[test]
fn log_empty_result() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_log_impl(dir.path(), None, None, false, &[], 20, None, &runner, &[])
        .unwrap()
        .into_content()
        .unwrap();

    assert_eq!(content, "No commits found matching the given filters.");
}

#[test]
fn log_empty_result_with_query_hints_at_content_param() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_log_impl(
        dir.path(),
        Some("c == '/'"),
        None,
        false,
        &[],
        20,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert!(content.starts_with("No commits found matching the given filters."));
    assert!(content.contains("`query` matches commit *messages* only"));
    assert!(content.contains("`content` parameter"));
}

#[test]
fn log_empty_result_with_content_has_no_hint() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::success("");

    let content = git_log_impl(
        dir.path(),
        None,
        Some("widget"),
        false,
        &[],
        20,
        None,
        &runner,
        &[],
    )
    .unwrap()
    .into_content()
    .unwrap();

    assert_eq!(content, "No commits found matching the given filters.");
}

#[test]
fn log_git_error() {
    let dir = tempdir().unwrap();
    let runner = MockProcessRunner::error("fatal: bad revision");

    let outcome = git_log_impl(dir.path(), None, None, false, &[], 20, None, &runner, &[]).unwrap();
    assert!(outcome.into_content().is_none(), "expected error outcome");
}

#[test]
fn format_uses_key_value_pairs() {
    let entries = vec![LogEntry {
        hash: "abc123full".into(),
        short_hash: "abc123".into(),
        author: "Alice".into(),
        date: "2024-06-01".into(),
        subject: "feat: add widget".into(),
    }];

    let output = format_log_entries(&entries).unwrap();
    assert!(output.contains("    hash: abc123full"));
    assert!(output.contains("    short_hash: abc123"));
    assert!(output.contains("    author: Alice"));
    assert!(output.contains("    subject: feat: add widget"));
    assert!(output.contains("  <commit>"));
    assert!(output.contains("  </commit>"));
}
