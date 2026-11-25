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

fn status(context: &str, state: &str, target_url: Option<&str>) -> Status {
    serde_json::from_value(json!({
        "context": context,
        "state": state,
        "description": null,
        "target_url": target_url,
    }))
    .unwrap()
}

fn no_statuses() -> CombinedStatus {
    CombinedStatus {
        total_count: 0,
        statuses: vec![],
    }
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
    let mut docs = run(7, "docs", "completed", Some("failure"));
    docs.html_url = Some(
        "https://github.com/dcdpr/jp/actions/runs/1/job/7"
            .parse()
            .unwrap(),
    );

    let summary = summarize_log(&docs, DOCS_LOG);

    assert_eq!(summary, indoc::indoc! {"
        Log of `docs` (job 7), 14 lines.
        Full log: https://github.com/dcdpr/jp/actions/runs/1/job/7

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

    assert_eq!(find_run(&runs, &[], "test (windows)").unwrap().id, 8);
    assert_eq!(find_run(&runs, &[], "7").unwrap().name, "docs");
}

/// Two workflows can each have a job of one name, and picking either would
/// return a valid log for the wrong job.
#[test]
fn an_ambiguous_job_name_lists_the_matching_ids() {
    let runs = vec![
        run(139_133_577_325, "changes", "completed", Some("success")),
        run(139_133_577_019, "changes", "completed", Some("failure")),
        run(7, "docs", "completed", Some("failure")),
    ];

    let error = find_run(&runs, &[], "changes").unwrap_err();

    assert_eq!(
        error.to_string(),
        "`changes` names 2 checks on the pull request's head commit: 139133577325, 139133577019. \
         Pass one of these IDs as `job`."
    );
    assert_eq!(
        find_run(&runs, &[], "139133577019").unwrap().conclusion,
        Some(CheckConclusion::Failure)
    );
}

#[test]
fn a_commit_status_has_no_log() {
    let statuses = vec![status(
        "merge-when-green",
        "pending",
        Some("https://github.com/dcdpr/jp/actions/runs/1"),
    )];

    let error = find_run(&[], &statuses, "merge-when-green").unwrap_err();

    assert_eq!(
        error.to_string(),
        "`merge-when-green` is a commit status, not a job, so it has no log. See \
         https://github.com/dcdpr/jp/actions/runs/1"
    );
}

#[test]
fn an_unknown_job_lists_the_checks_there_are() {
    let runs = vec![
        run(8, "test (windows)", "in_progress", None),
        run(7, "docs", "completed", Some("failure")),
        run(6, "docs", "completed", Some("success")),
    ];

    let error = find_run(&runs, &[], "lint").unwrap_err();

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

    let xml = format_checks(42, "abc123", runs, no_statuses(), vec![
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

/// A gate written through the commit statuses API, such as `merge-when-green`,
/// is not a check run; a listing without it would read as green while the gate
/// is still pending.
#[test]
fn commit_statuses_are_listed_and_tallied_with_the_checks() {
    let runs = CheckRuns {
        total_count: 1,
        check_runs: vec![run(1, "merge-label-gate", "completed", Some("success"))],
    };
    let statuses = CombinedStatus {
        total_count: 2,
        statuses: vec![
            status("coverage/coveralls", "success", None),
            status(
                "merge-when-green",
                "pending",
                Some("https://github.com/dcdpr/jp/actions/runs/1"),
            ),
        ],
    };

    let xml = format_checks(42, "abc123", runs, statuses, vec![vec![]]).unwrap();

    assert_eq!(xml, indoc::indoc! {"
        ```xml
        <Checks>
          <number>42</number>
          <sha>abc123</sha>
          <failed>0</failed>
          <pending>1</pending>
          <passed>2</passed>
          <check>
            <id>1</id>
            <name>merge-label-gate</name>
            <status>completed</status>
            <conclusion>success</conclusion>
          </check>
          <commit_status>
            <context>merge-when-green</context>
            <state>pending</state>
            <target_url>https://github.com/dcdpr/jp/actions/runs/1</target_url>
          </commit_status>
          <commit_status>
            <context>coverage/coveralls</context>
            <state>success</state>
          </commit_status>
        </Checks>
        ```"});
}

/// GitHub's schema lets an annotation's level and message be null; the check
/// stays listed with whatever the annotation does carry.
#[test]
fn an_annotation_without_level_or_message_keeps_its_check() {
    let mut docs = run(3, "docs", "completed", Some("failure"));
    docs.html_url = Some(
        "https://github.com/dcdpr/jp/actions/runs/1/job/3"
            .parse()
            .unwrap(),
    );
    let runs = CheckRuns {
        total_count: 1,
        check_runs: vec![docs],
    };
    let annotation: Annotation = serde_json::from_value(json!({
        "path": "src/lib.rs",
        "start_line": 4,
        "end_line": 6,
        "annotation_level": null,
        "title": null,
        "message": null
    }))
    .unwrap();

    let xml = format_checks(42, "abc123", runs, no_statuses(), vec![vec![annotation]]).unwrap();

    assert_eq!(xml, indoc::indoc! {"
        ```xml
        <Checks>
          <number>42</number>
          <sha>abc123</sha>
          <failed>1</failed>
          <pending>0</pending>
          <passed>0</passed>
          <check>
            <id>3</id>
            <name>docs</name>
            <html_url>https://github.com/dcdpr/jp/actions/runs/1/job/3</html_url>
            <status>completed</status>
            <conclusion>failure</conclusion>
            <annotation>
              <location>src/lib.rs:4-6</location>
            </annotation>
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

    let xml = format_checks(42, "abc123", runs, no_statuses(), vec![vec![]]).unwrap();

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
