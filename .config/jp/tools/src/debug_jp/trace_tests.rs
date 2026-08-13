use std::time::Duration;

use super::*;
use crate::debug_jp::util::launch::{LaunchResult, MockLauncher, Termination};

fn launched(stderr: impl Into<String>, termination: Termination) -> LaunchResult {
    LaunchResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: stderr.into(),
        wall_duration: Duration::from_secs(1),
        termination,
    }
}

fn spec(workspace: &Utf8Path) -> LaunchSpec {
    LaunchSpec {
        binary: "/does/not/run".into(),
        args: vec!["c".to_owned(), "fork".to_owned()],
        working_dir: workspace.to_owned(),
        env: vec![],
    }
}

#[test]
fn renders_report_from_launched_trace_log() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    // Pre-stage the trace log jp would have flushed, and point the marker at it.
    let trace_log = root.join("trace-src.jsonl");
    std::fs::write(&trace_log, "").unwrap();
    let launcher = MockLauncher::returning(launched(
        format!("{TRACE_PATH_PREFIX}{trace_log}\n"),
        Termination::Exited,
    ));

    let outcome = execute(
        root,
        &spec(root),
        Level::Info,
        None,
        None,
        &launcher,
        Timeouts::DEFAULT,
    )
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("expected a success outcome");
    };
    assert!(!content.is_empty());
    // A natural exit carries no shutdown-warning banner.
    assert!(!content.contains("[!WARNING]"));
    // The report and copied streams landed under tmp/profiling.
    assert!(root.join("tmp/profiling").exists());
}

#[test]
fn renders_report_from_json_format_marker_line() {
    // `--format json` / `json-pretty` makes jp emit a `{"trace_log": ...}`
    // object on stderr instead of the text marker line.
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    let trace_log = root.join("trace-src.jsonl");
    std::fs::write(&trace_log, "").unwrap();
    // Built via `serde_json::json!` (like the real jp code) rather than
    // string interpolation, so the path's backslashes on Windows get
    // properly JSON-escaped.
    let marker = serde_json::json!({ "trace_log": trace_log.as_str() });
    let launcher = MockLauncher::returning(launched(format!("{marker}\n"), Termination::Exited));

    let outcome = execute(
        root,
        &spec(root),
        Level::Info,
        None,
        None,
        &launcher,
        Timeouts::DEFAULT,
    )
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("expected a success outcome");
    };
    assert!(!content.is_empty());
    assert!(root.join("tmp/profiling").exists());
}

#[test]
fn renders_combined_report_for_command_sequence() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    // Both commands resolve to the same pre-staged trace log via the mock
    // launcher; the point under test is the sequencing and combined render.
    let trace_log = root.join("trace-src.jsonl");
    std::fs::write(&trace_log, "").unwrap();
    let launcher = MockLauncher::returning(launched(
        format!("{TRACE_PATH_PREFIX}{trace_log}\n"),
        Termination::Exited,
    ));

    let specs = vec![
        LaunchSpec {
            binary: "/does/not/run".into(),
            args: vec!["c".to_owned(), "new".to_owned()],
            working_dir: root.to_owned(),
            env: vec![],
        },
        LaunchSpec {
            binary: "/does/not/run".into(),
            args: vec!["q".to_owned(), "continue".to_owned()],
            working_dir: root.to_owned(),
            env: vec![],
        },
    ];

    let outcome = execute_sequence(
        root,
        &specs,
        Level::Info,
        None,
        None,
        &launcher,
        Timeouts::DEFAULT,
    )
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("expected a success outcome");
    };
    assert!(
        content.contains("## Command 1/2: `jp c new`"),
        "got:\n{content}"
    );
    assert!(
        content.contains("## Command 2/2: `jp q continue`"),
        "got:\n{content}"
    );
    // Per-command sidecar files are listed with their command index.
    assert!(content.contains("- Command 1 trace:"));
    assert!(content.contains("- Command 2 trace:"));
    // Each command wrote its own labelled sidecars.
    let profiling = root.join("tmp/profiling");
    assert!(profiling.exists());
}

/// A launcher that emulates jp's `--log-file` handling: it writes `content` to
/// the path named by the injected flag, then returns a canned result.
/// Panics when the tool did not inject the flag, pinning the injection itself.
struct LogFileWritingLauncher {
    content: &'static str,
    result: LaunchResult,
}

impl Launcher for LogFileWritingLauncher {
    fn run(
        &self,
        spec: &LaunchSpec,
        _timeouts: Timeouts,
        on_spawn: &mut dyn FnMut(u32),
    ) -> Result<LaunchResult, Error> {
        on_spawn(0);
        assert_eq!(
            spec.args.first().map(String::as_str),
            Some("--log-file"),
            "expected `--log-file` to be injected before the user args"
        );
        std::fs::write(&spec.args[1], self.content).unwrap();
        Ok(self.result.clone())
    }
}

#[test]
fn force_killed_run_still_renders_from_pinned_log_file() {
    // A force-killed jp prints no trace-path marker on stderr, but the log
    // pinned via `--log-file` holds every event written up to the kill.
    // The run must succeed from that file alone, with the force-kill banner.
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let launcher = LogFileWritingLauncher {
        content: "{\"timestamp\":\"2026-07-10T10:00:00.000Z\",\"level\":\"INFO\",\"fields\":{\"\
                  message\":\"event before kill\"},\"target\":\"jp_cli\"}\n",
        result: launched("no marker here\n", Termination::Forced),
    };

    let outcome = execute(
        root,
        &spec(root),
        Level::Info,
        None,
        None,
        &launcher,
        Timeouts::DEFAULT,
    )
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("expected a success outcome");
    };
    assert!(content.contains("[!WARNING]"), "got:\n{content}");
    assert!(content.contains("force-killed"), "got:\n{content}");
    // The event came from the pinned log file, not the (absent) marker path.
    assert!(content.contains("event before kill"), "got:\n{content}");
}

#[test]
fn force_killed_without_marker_reports_note() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let launcher = MockLauncher::returning(launched(
        "unrelated stderr without the marker\n",
        Termination::Forced,
    ));

    let error = execute(
        root,
        &spec(root),
        Level::Info,
        None,
        None,
        &launcher,
        Timeouts::DEFAULT,
    )
    .unwrap_err()
    .to_string();

    // The force-kill note is folded into the missing-marker error.
    assert!(error.contains("force-killed"), "got: {error}");
    assert!(error.contains(TRACE_PATH_PREFIX), "got: {error}");
}
