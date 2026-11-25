use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn run(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> CheckRun {
    serde_json::from_value(json!({
        "id": id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "started_at": null,
        "completed_at": null,
        "html_url": null,
        "output": { "title": null, "summary": null, "annotations_count": 0 },
        "app": { "slug": "github-actions" }
    }))
    .unwrap()
}

/// A `docs` job log as GitHub serves it: a byte-order mark, a timestamp on
/// every line, ANSI colors, and cargo progress lines between diagnostics.
const DOCS_LOG: &str = concat!(
    "\u{feff}2026-11-25T10:00:00.0000000Z ##[group]Run just docs-ci\n",
    "2026-11-25T10:00:00.1000000Z just docs-ci\n",
    "2026-11-25T10:00:00.2000000Z ##[endgroup]\n",
    "2026-11-25T10:00:01.0000000Z \x1b[1m\x1b[32m Documenting\x1b[0m jp-review-ui v0.1.0\n",
    "2026-11-25T10:00:02.0000000Z \x1b[1m\x1b[31merror\x1b[0m: unresolved link to `layout::site`\n",
    "2026-11-25T10:00:02.0000000Z   --> crates/review/ui/src/compose.rs:13:7\n",
    "2026-11-25T10:00:02.0000000Z    |\n",
    "2026-11-25T10:00:02.0000000Z 13 | //! [`layout::site`] says which.\n",
    "2026-11-25T10:00:02.0000000Z    |       ^^^^^^^^^^^^ no item named `layout` in scope\n",
    "2026-11-25T10:00:02.0000000Z \n",
    "2026-11-25T10:00:02.0000000Z error: could not document `jp-review-ui`\n",
    "2026-11-25T10:00:02.0000000Z warning: build failed, waiting for other jobs to finish...\n",
    "2026-11-25T10:00:03.0000000Z  Documenting jp_tombmap v0.1.0\n",
    "2026-11-25T10:00:04.0000000Z ##[error]Process completed with exit code 101.\n",
);

#[test]
fn a_log_is_reduced_to_its_diagnostics_and_tail() {
    let summary = summarize_log(&run(7, "docs", "completed", Some("failure")), DOCS_LOG);

    assert_eq!(summary, indoc::indoc! {"
        Log of `docs` (job 7), 14 lines.

        Diagnostics:
        ```text
        error: unresolved link to `layout::site`
          --> crates/review/ui/src/compose.rs:13:7
           |
        13 | //! [`layout::site`] says which.
           |       ^^^^^^^^^^^^ no item named `layout` in scope

        error: could not document `jp-review-ui`

        warning: build failed, waiting for other jobs to finish...

        ##[error]Process completed with exit code 101.
        ```

        Last 40 lines:
        ```text
        ##[group]Run just docs-ci
        just docs-ci
        ##[endgroup]
         Documenting jp-review-ui v0.1.0
        error: unresolved link to `layout::site`
          --> crates/review/ui/src/compose.rs:13:7
           |
        13 | //! [`layout::site`] says which.
           |       ^^^^^^^^^^^^ no item named `layout` in scope

        error: could not document `jp-review-ui`
        warning: build failed, waiting for other jobs to finish...
         Documenting jp_tombmap v0.1.0
        ##[error]Process completed with exit code 101.
        ```"});
}

#[test]
fn a_log_without_diagnostics_returns_its_tail() {
    let log = concat!(
        "2026-11-25T10:00:00.0000000Z Summary [ 1.000s] 2 tests run: 2 passed\n",
        "2026-11-25T10:00:01.0000000Z\n",
        "2026-11-25T10:00:02.0000000Z Post job cleanup.\n",
    );

    let summary = summarize_log(&run(9, "test", "completed", Some("success")), log);

    assert_eq!(summary, indoc::indoc! {"
        Log of `test` (job 9), 3 lines. It has no compiler diagnostics.

        Last 40 lines:
        ```text
        Summary [ 1.000s] 2 tests run: 2 passed

        Post job cleanup.
        ```"});
}

#[test]
fn the_tail_holds_only_the_last_lines_of_a_long_log() {
    let mut log = String::new();
    for n in 1..=50 {
        log.push_str(&format!("2026-11-25T10:00:00.0000000Z line {n}\n"));
    }

    let summary = summarize_log(&run(9, "test", "completed", Some("success")), &log);

    let tail = summary
        .split_once("```text\n")
        .map(|(_, tail)| tail)
        .unwrap();
    assert!(tail.starts_with("line 11\n"), "tail: {tail}");
    assert!(tail.ends_with("line 50\n```"), "tail: {tail}");
}

#[test]
fn diagnostics_start_at_compiler_errors_warnings_and_panics() {
    assert!(starts_diagnostic("error: could not compile `foo`"));
    assert!(starts_diagnostic("error[E0308]: mismatched types"));
    assert!(starts_diagnostic("warning: unused import: `Foo`"));
    assert!(starts_diagnostic(
        "thread 'tests::a' (12) panicked at src/lib_tests.rs:4:5:"
    ));

    assert!(!starts_diagnostic("errors: 3"));
    assert!(!starts_diagnostic("  error: indented"));
    assert!(!starts_diagnostic("warnings are errors"));
}

#[test]
fn a_job_is_found_by_name_or_id() {
    let runs = vec![
        run(7, "docs", "completed", Some("failure")),
        run(8, "test (windows)", "in_progress", None),
    ];

    assert_eq!(find_run(&runs, "test (windows)").unwrap().id, 8);
    assert_eq!(find_run(&runs, "7").unwrap().name, "docs");
}

#[test]
fn an_unknown_job_lists_the_checks_there_are() {
    let runs = vec![
        run(8, "test (windows)", "in_progress", None),
        run(7, "docs", "completed", Some("failure")),
    ];

    let error = find_run(&runs, "lint").unwrap_err();

    assert_eq!(
        error.to_string(),
        "No check named `lint` on the pull request's head commit. Its checks: `docs`, `test \
         (windows)`"
    );
}

#[test]
fn checks_list_failures_first_with_their_annotations() {
    let runs = CheckRuns {
        total_count: 4,
        check_runs: vec![
            run(1, "lint", "completed", Some("success")),
            run(2, "test", "in_progress", None),
            run(3, "docs", "completed", Some("failure")),
            run(4, "deny", "completed", Some("skipped")),
        ],
    };
    let annotation: Annotation = serde_json::from_value(json!({
        "path": "crates/review/ui/src/compose.rs",
        "start_line": 13,
        "end_line": 13,
        "annotation_level": "failure",
        "title": "",
        "message": "unresolved link to `layout::site`"
    }))
    .unwrap();

    let xml = format_checks(42, "abc123", runs, vec![
        vec![],
        vec![],
        vec![annotation],
        vec![],
    ])
    .unwrap();

    assert_eq!(xml, indoc::indoc! {"
        ```xml
        <Checks>
          <number>42</number>
          <sha>abc123</sha>
          <failed>1</failed>
          <pending>1</pending>
          <passed>2</passed>
          <check>
            <id>3</id>
            <name>docs</name>
            <status>completed</status>
            <conclusion>failure</conclusion>
            <annotation>
              <location>crates/review/ui/src/compose.rs:13</location>
              <level>failure</level>
              <message>unresolved link to `layout::site`</message>
            </annotation>
          </check>
          <check>
            <id>2</id>
            <name>test</name>
            <status>in_progress</status>
          </check>
          <check>
            <id>4</id>
            <name>deny</name>
            <status>completed</status>
            <conclusion>skipped</conclusion>
          </check>
          <check>
            <id>1</id>
            <name>lint</name>
            <status>completed</status>
            <conclusion>success</conclusion>
          </check>
        </Checks>
        ```"});
}

/// A commit with more check runs than one page holds says so, so a short
/// listing doesn't read as the whole picture.
#[test]
fn a_truncated_listing_reports_the_total() {
    let runs = CheckRuns {
        total_count: 150,
        check_runs: vec![run(1, "lint", "completed", Some("success"))],
    };

    let xml = format_checks(42, "abc123", runs, vec![vec![]]).unwrap();

    assert_eq!(xml, indoc::indoc! {"
        ```xml
        <Checks>
          <number>42</number>
          <sha>abc123</sha>
          <failed>0</failed>
          <pending>0</pending>
          <passed>1</passed>
          <total_count>150</total_count>
          <check>
            <id>1</id>
            <name>lint</name>
            <status>completed</status>
            <conclusion>success</conclusion>
          </check>
        </Checks>
        ```"});
}
