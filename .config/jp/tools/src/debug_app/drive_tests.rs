use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::Outcome;
use serde_json::json;

use super::{Reads, cap, header, result_line, run};
use crate::{
    debug_app::{
        session::{Console, Session, Slot},
        steps::parse,
        tree::Options,
    },
    util::{
        paths::shortenings_from,
        runner::{ExitCode, MockProcessRunner, ProcessOutput},
    },
};

/// The app before anything is selected.
const BEFORE: &str = r#"{
  "role": "AXApplication",
  "children": [{"role": "AXRow", "identifier": "sidebar.row.a", "children": []}]
}"#;

/// The same app with the row selected.
const AFTER: &str = r#"{
  "role": "AXApplication",
  "children": [
    {"role": "AXRow", "identifier": "sidebar.row.a", "focused": true, "children": []}
  ]
}"#;

/// What `jpdrive act` answers for a successful `select`, keys sorted as its
/// encoder sorts them.
const SELECTED: &str = r#"{
  "confirmed": true,
  "identifier": "sidebar.row.a",
  "role": "AXRow",
  "step": "select"
}"#;

/// The slot every test in this file shares, so paths are predictable.
fn dir_for(root: &Utf8Path) -> Utf8PathBuf {
    Session::dir(root, &Slot::fixed("test"))
}

/// A session pointing at this process, so it resolves as running.
fn record_session(root: &Utf8Path) -> Session {
    let dir = dir_for(root);
    let session = Session {
        pid: std::process::id(),
        bundle: Utf8Path::new("/tmp/JP.app").to_owned(),
        configuration: "Debug".to_owned(),
        workspace: Utf8Path::new("/repo/tmp/debug-app/workspace").to_owned(),
        state_dir: dir.join("state"),
        user_data_dir: dir.join("data"),
        stdout: Console::new(dir.join("console.out")),
        stderr: Console::new(dir.join("console.err")),
        trace: Console::new(dir.join("state/trace.jsonl")),
        reported_footprint_mb: None,
        dsym: None,
        allocation_stacks: false,
    };

    session.store(&dir).unwrap();
    fs::create_dir_all(&session.state_dir).unwrap();
    fs::write(session.pid_path(), format!("{}\n", session.pid)).unwrap();

    session
}

/// A file where `driver::locate` looks for the built binary.
fn fake_driver(root: &Utf8Path) -> Utf8PathBuf {
    let dir = root.join("bin");
    fs::create_dir_all(&dir).unwrap();
    let bin = dir.join("jpdrive");
    fs::write(&bin, "").unwrap();
    bin
}

#[test]
fn reports_the_result_and_the_delta_of_each_step() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let session = record_session(root);
    let bin = fake_driver(root);

    // Present before the run, so the first step's console delta holds it.
    fs::write(&session.stderr.path, "*** constraint complaint\n").unwrap();

    let pid = session.pid.to_string();
    let read = ["tree", "--pid", &pid, "--max-siblings", "0"];
    let select = [
        "act",
        "--pid",
        &pid,
        "--json",
        r#"{"select":{"identifier":"sidebar.row.a"}}"#,
    ];

    let runner = MockProcessRunner::builder()
        .expect("just")
        .args(&["build-drive"])
        .returns_success("")
        .expect("swift")
        .returns_success(format!("{}\n", bin.parent().unwrap()))
        // The baseline reading.
        .expect(bin.as_str())
        .args(&read)
        .returns_success(BEFORE)
        // Step 1: select, then the reading after it.
        .expect(bin.as_str())
        .args(&select)
        .returns_success(SELECTED)
        .expect(bin.as_str())
        .args(&read)
        .returns_success(AFTER)
        // Step 2: snapshot, which reads without acting.
        .expect(bin.as_str())
        .args(&read)
        .returns_success(AFTER);

    let steps = parse(&json!([
        {"select": {"identifier": "sidebar.row.a"}},
        {"snapshot": {}}
    ]))
    .unwrap();

    let outcome = run(
        root,
        &dir_for(root),
        &steps,
        &Options::default(),
        Reads::EveryStep,
        &runner,
    )
    .unwrap();

    assert_eq!(outcome, Outcome::Success {
        content: "Ran all 2 steps against the app on `/repo/tmp/debug-app/workspace`.\n\nReadings \
                  cover the whole application.\n\n### 1. select \
                  {\"identifier\":\"sidebar.row.a\"}\n\nstep=select identifier=sidebar.row.a \
                  role=AXRow confirmed=true\n\nTree delta:\n\n```diff\n@@ -1,2 +1,2 @@\n \
                  AXApplication\n-  AXRow #sidebar.row.a\n+  AXRow #sidebar.row.a \
                  [focused]\n```\n\nConsole (stderr):\n\n```\n*** constraint \
                  complaint\n```\n\n### 2. snapshot\n\nThe tree did not change.\n\nWhat each step \
                  cost the app: `debug_app_profile` with `mode: \"report\"`.\n"
            .to_owned()
    });
}

/// The whole point of `reads: "none"`: not one tree read is issued, so the app
/// pays nothing for being watched.
///
/// A read is the driver's own work but the *app* answers it, on the thread it
/// draws on, and an unscoped one walks every element it publishes.
/// Against a transcript that is thousands of elements, which makes a
/// measurement of the app's own cost mostly a measurement of the reads — in
/// proportion to the content on screen, the usual thing under study.
///
/// The mock fails the run on any command it was not told to expect, so a read
/// slipping back in is a failure here rather than a slower number somewhere
/// else.
#[test]
fn reads_none_issues_no_tree_reads_at_all() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let session = record_session(root);
    let bin = fake_driver(root);

    let select = [
        "act",
        "--pid",
        &session.pid.to_string(),
        "--json",
        r#"{"select":{"identifier":"sidebar.row.a"}}"#,
    ];

    // Two build lookups and the one act. No `tree` call, in either direction.
    let runner = MockProcessRunner::builder()
        .expect("just")
        .args(&["build-drive"])
        .returns_success("")
        .expect("swift")
        .returns_success(format!("{}\n", bin.parent().unwrap()))
        .expect(bin.as_str())
        .args(&select)
        .returns_success(SELECTED);

    let steps = parse(&json!([{"select": {"identifier": "sidebar.row.a"}}])).unwrap();

    let outcome = run(
        root,
        &dir_for(root),
        &steps,
        &Options::default(),
        Reads::None,
        &runner,
    )
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("expected success: {outcome:?}");
    };

    assert!(content.contains("The tree was not read"), "{content}");
    assert!(!content.contains("Tree delta"), "{content}");
    assert!(!content.contains("did not change"), "{content}");
}

#[test]
fn reads_rejects_a_value_that_is_neither_setting() {
    let error = Reads::parse(Some("sometimes")).unwrap_err().to_string();

    assert!(
        error.starts_with("`reads` takes `every_step` or `none`"),
        "{error}"
    );
}

#[test]
fn reads_defaults_to_reading_after_every_step() {
    assert_eq!(Reads::parse(None).unwrap(), Reads::EveryStep);
    assert_eq!(Reads::parse(Some("every_step")).unwrap(), Reads::EveryStep);
    assert_eq!(Reads::parse(Some("none")).unwrap(), Reads::None);
}

/// A scoped reading that matches nothing is a reading, not a failed run: a view
/// part-way through loading holds none of the identifiers it will hold a moment
/// later.
/// Erroring here loses the report for every step that already ran.
#[test]
fn a_reading_that_matches_nothing_is_reported_rather_than_fatal() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    record_session(root);
    let bin = fake_driver(root);

    let no_match = r#"{"error": {"kind": "identifier_not_found", "message": "no element's identifier begins with sidebar.list", "hint": "drop --identifier to see what the application reports"}}"#;

    let pid = std::process::id().to_string();
    let read = [
        "tree",
        "--pid",
        &pid,
        "--max-siblings",
        "0",
        "--identifier",
        "sidebar.list",
    ];

    let runner = MockProcessRunner::builder()
        .expect("just")
        .args(&["build-drive"])
        .returns_success("")
        .expect("swift")
        .returns_success(format!("{}\n", bin.parent().unwrap()))
        .expect(bin.as_str())
        .args(&read)
        .returns_success(BEFORE)
        // A `menu` step synthesizes input, so the run borrows what is in front
        // and where the pointer is, and hands both back when it ends.
        .expect(bin.as_str())
        .args(&["frontmost"])
        .returns_success(r#"{"bundle_id":"com.apple.Terminal"}"#)
        .expect(bin.as_str())
        .args(&["pointer"])
        .returns_success(r#"{"x":10,"y":20}"#)
        .expect(bin.as_str())
        .returns_success(
            r#"{"step": "menu", "identifier": "File > New Window", "role": "AXMenuItem"}"#,
        )
        .expect(bin.as_str())
        .args(&read)
        .returns(ProcessOutput {
            stdout: no_match.to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        })
        .expect(bin.as_str())
        .args(&["frontmost", "--set", "com.apple.Terminal"])
        .returns_success("{}")
        .expect(bin.as_str())
        .args(&["pointer", "--set", "10,20"])
        .returns_success("{}");

    let steps = parse(&json!([{"menu": {"path": ["File", "New Window"]}}])).unwrap();
    let opts = Options {
        identifier: Some("sidebar.list".to_owned()),
        ..Options::default()
    };

    let outcome = run(
        root,
        &dir_for(root),
        &steps,
        &opts,
        Reads::EveryStep,
        &runner,
    )
    .unwrap();

    assert_eq!(outcome, Outcome::Success {
        content: "Ran the step against the app on `/repo/tmp/debug-app/workspace`.\n\nReadings \
                  cover the elements under `sidebar.list`.\n\n### 1. menu \
                  {\"path\":[\"File\",\"New Window\"]}\n\nstep=menu identifier=File > New Window \
                  role=AXMenuItem\n\nTree delta:\n\n```diff\n@@ -1,2 +1 @@\n-AXApplication\n-  \
                  AXRow #sidebar.row.a\n+(nothing matched `sidebar.list`)\n```\n\nWhat each step \
                  cost the app: `debug_app_profile` with `mode: \"report\"`.\n"
            .to_owned()
    });
}

/// A wait that never resolves is the common failure, and the report has to say
/// what the tree held instead of the identifier that was waited on.
#[test]
fn stops_at_the_first_failing_step_and_shows_the_tree() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    record_session(root);
    let bin = fake_driver(root);

    let refusal = r#"{"error": {"kind": "timeout", "message": "transcript.scroll did not appear within 5000ms (48 attempts over 5003ms)", "hint": "one attempt exhausted the timeout; scope the search with `under`"}}"#;

    let pid = std::process::id().to_string();
    let read = ["tree", "--pid", &pid, "--max-siblings", "0"];
    let wait = [
        "act",
        "--pid",
        &pid,
        "--json",
        r#"{"wait_for":{"identifier":"transcript.scroll"}}"#,
    ];

    let runner = MockProcessRunner::builder()
        .expect("just")
        .args(&["build-drive"])
        .returns_success("")
        .expect("swift")
        .returns_success(format!("{}\n", bin.parent().unwrap()))
        .expect(bin.as_str())
        .args(&read)
        .returns_success(BEFORE)
        // Step 1 fails, so the reading after it is the last thing that runs.
        .expect(bin.as_str())
        .args(&wait)
        .returns(ProcessOutput {
            stdout: refusal.to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        })
        .expect(bin.as_str())
        .args(&read)
        .returns_success(BEFORE);

    let steps = parse(&json!([
        {"wait_for": {"identifier": "transcript.scroll"}},
        {"press": {"identifier": "transcript.event.0"}},
        {"snapshot": {}}
    ]))
    .unwrap();

    let outcome = run(
        root,
        &dir_for(root),
        &steps,
        &Options::default(),
        Reads::EveryStep,
        &runner,
    )
    .unwrap();

    let Outcome::Error { message, .. } = outcome else {
        panic!("a failing step must not report success: {outcome:?}");
    };

    assert_eq!(
        message,
        "Ran 0 of 3 steps against the app on `/repo/tmp/debug-app/workspace`, then stopped at \
         step 1. The remaining 2 steps were not run.\n\nReadings cover the whole \
         application.\n\n### 1. wait_for {\"identifier\":\"transcript.scroll\"}\n\nFailed: \
         `jpdrive act` failed (timeout): transcript.scroll did not appear within 5000ms (48 \
         attempts over 5003ms)\n\nHint: one attempt exhausted the timeout; scope the search with \
         `under`\n\nThe tree at the failure:\n\n```\nAXApplication\n  AXRow #sidebar.row.a\n```\n"
    );
}

/// The driver owns the result document, so every field it reports is shown
/// rather than a chosen few.
#[test]
fn a_result_line_shows_every_field_the_driver_reported() {
    let line = result_line(
        r#"{"committed": false, "confirmed": true, "identifier": "search.field", "role": "AXTextField", "step": "type"}"#,
    );

    assert_eq!(
        line,
        "step=type identifier=search.field role=AXTextField committed=false confirmed=true"
    );
}

/// A driver that answered something unparseable is quoted rather than dropped.
#[test]
fn a_result_line_falls_back_to_the_raw_output() {
    assert_eq!(result_line("  not json\n"), "not json");
}

#[test]
fn a_capped_block_names_what_it_left_out() {
    let text = (1..=250)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");

    let capped = cap(&text);

    assert!(capped.starts_with("line 1\nline 2\n"), "{capped}");
    assert!(
        capped.ends_with("line 200\n\n[50 more lines, not shown]\n"),
        "{capped}"
    );
}

/// A block that fits is passed through, but always ends in a newline so it does
/// not close its fence on the same line.
#[test]
fn a_short_block_gains_only_a_trailing_newline() {
    assert_eq!(cap("one\ntwo"), "one\ntwo\n");
    assert_eq!(cap("one\ntwo\n"), "one\ntwo\n");
}

/// The header names no process id: a relaunch changes it, and two runs of the
/// same list against the same state should produce the same report.
#[test]
fn the_header_names_the_scope_and_no_process_id() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let session = record_session(workspace.path());
    let opts = Options {
        identifier: Some("sidebar.".to_owned()),
        ..Options::default()
    };

    // Relative to the repository: a report is meant to be pasteable into an
    // issue, and an absolute path here says whose machine produced it.
    let shortenings = shortenings_from(Utf8Path::new("/repo"), Some("/Users/jean"), None, None);

    assert_eq!(
        header(&session, 9, 9, &opts, Reads::EveryStep, false, &shortenings),
        "Ran all 9 steps against the app on `tmp/debug-app/workspace`.\n\nReadings cover the \
         elements under `sidebar.`.\n"
    );

    assert_eq!(
        header(&session, 3, 9, &opts, Reads::EveryStep, true, &shortenings),
        "Ran 3 of 9 steps against the app on `tmp/debug-app/workspace`, then stopped at step 4. \
         The remaining 5 steps were not run.\n\nReadings cover the elements under `sidebar.`.\n"
    );
}
