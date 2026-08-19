use std::{
    fs,
    process::Command,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use camino::Utf8Path;

use super::run;
use crate::debug_app::session::{
    Console, RealSignals, Session, Signal, Signals, Slot, pid_is_alive,
};

/// A slot every test in this file shares, so paths are predictable.
fn dir_for(root: &Utf8Path) -> camino::Utf8PathBuf {
    Session::dir(root, &Slot::fixed("test"))
}

/// Above macOS's default maximum pid, so no process can hold it.
const DEAD_PID: u32 = 4_000_000;

/// Long enough for a real process to notice a signal, short enough not to pad
/// the suite.
const GRACE: Duration = Duration::from_secs(2);

/// Start a process and reap it on a side thread, returning its pid.
///
/// The reaper is what makes the process observably disappear.
/// A killed child with nobody waiting on it becomes a zombie, and a zombie
/// still answers `kill(pid, 0)`, so the code under test would poll a process
/// that never goes away.
/// Production does not have this problem: `open(1)` leaves the app parented to
/// launchd, which reaps it.
fn spawn_reaped(program: &str, args: &[&str]) -> u32 {
    let child = Command::new(program).args(args).spawn().unwrap();
    let pid = child.id();

    thread::spawn(move || {
        let mut child = child;
        drop(child.wait());
    });

    pid
}

/// A [`Signals`] that records what it was sent and only dies on `SIGKILL`.
///
/// Replaces a real process for the escalation ladder, and pins the exact
/// sequence of signals — which no real fixture can assert, since a process
/// cannot report what it ignored.
struct IgnoresTerm {
    sent: Mutex<Vec<Signal>>,
    alive: AtomicBool,
    dies_on: Signal,
}

impl IgnoresTerm {
    fn dying_on(dies_on: Signal) -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            alive: AtomicBool::new(true),
            dies_on,
        }
    }

    fn sent(&self) -> Vec<Signal> {
        self.sent.lock().unwrap().clone()
    }
}

impl Signals for IgnoresTerm {
    fn send(&self, _pid: u32, signal: Signal) {
        self.sent.lock().unwrap().push(signal);
        if signal == self.dies_on {
            self.alive.store(false, Ordering::SeqCst);
        }
    }

    fn is_alive(&self, _pid: u32) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

/// Record a session for `pid`, including the pid file the app writes.
fn record(root: &Utf8Path, pid: u32) -> Session {
    let dir = dir_for(root);
    let session = Session {
        pid,
        bundle: "/derived/JP.app".into(),
        configuration: "Debug".to_owned(),
        workspace: root.join("workspace"),
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
    fs::write(session.pid_path(), format!("{pid}\n")).unwrap();
    session
}

fn content(outcome: jp_tool::Outcome) -> String {
    match outcome {
        jp_tool::Outcome::Success { content } => content,
        jp_tool::Outcome::Error { message, .. } => message,
        other @ jp_tool::Outcome::NeedsInput { .. } => {
            panic!("unexpected outcome: {other:?}")
        }
    }
}

#[test]
fn errors_without_a_recorded_session() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    let outcome = run(root, &dir_for(root), GRACE, &RealSignals).unwrap();

    assert_eq!(
        content(outcome),
        format!(
            "No app session recorded at {}, so there is nothing to stop.",
            Session::path(&dir_for(root))
        )
    );
}

/// Quitting an app that already died is not a failure, but it must say so
/// rather than claim to have stopped anything.
#[test]
fn reports_an_app_that_was_already_gone() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let session = record(root, DEAD_PID);

    let report = content(run(root, &dir_for(root), GRACE, &RealSignals).unwrap());

    assert!(
        report.starts_with(
            "The app recorded as pid 4000000 was already gone. Cleared the session record.\n"
        ),
        "unexpected report: {report}"
    );
    assert!(!Session::path(&dir_for(root)).exists());
    assert!(
        session.state_dir.is_dir(),
        "the state directory has to survive for a relaunch"
    );
}

#[test]
fn stops_a_running_process_with_sigterm() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    let session = record(root, spawn_reaped("sleep", &["30"]));

    let report = content(run(root, &dir_for(root), GRACE, &RealSignals).unwrap());

    assert!(
        report.starts_with(&format!(
            "Stopped the app (pid {}) with SIGTERM.\n",
            session.pid
        )),
        "unexpected report: {report}"
    );
    assert!(!pid_is_alive(session.pid));
    assert!(!Session::path(&dir_for(root)).exists());
}

/// The escalation only exists for an app that ignores `SIGTERM`, so the fixture
/// has to actually ignore it.
/// A process that exits on the first signal would take the same branch as the
/// test above and prove nothing.
#[test]
fn kills_a_process_that_ignores_sigterm() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();

    // The recorded pid is this process, which is alive, so the run reaches the
    // ladder. Nothing is signalled for real: the fake stands in for the app.
    let session = record(root, std::process::id());
    let signals = IgnoresTerm::dying_on(Signal::Kill);

    let report = content(run(root, &dir_for(root), Duration::from_millis(200), &signals).unwrap());

    assert!(
        report.starts_with(&format!(
            "The app (pid {}) ignored SIGTERM and was killed.",
            session.pid
        )),
        "unexpected report: {report}"
    );
    assert_eq!(signals.sent(), vec![Signal::Term, Signal::Kill]);
}

/// The other half of the ladder: an app that goes on `SIGTERM` must never be
/// killed.
/// Only the fake can assert the absence of that second signal.
#[test]
fn does_not_escalate_when_sigterm_is_enough() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let session = record(root, std::process::id());
    let signals = IgnoresTerm::dying_on(Signal::Term);

    let report = content(run(root, &dir_for(root), GRACE, &signals).unwrap());

    assert!(
        report.starts_with(&format!(
            "Stopped the app (pid {}) with SIGTERM.\n",
            session.pid
        )),
        "unexpected report: {report}"
    );
    assert_eq!(signals.sent(), vec![Signal::Term]);
}

/// The offsets live in the record being deleted, so the console has to be read
/// before it goes.
#[test]
fn returns_the_console_written_since_the_last_call() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let session = record(root, DEAD_PID);
    fs::write(
        &session.stderr.path,
        "reentrant operation in its NSTableView delegate\n",
    )
    .unwrap();

    let report = content(run(root, &dir_for(root), GRACE, &RealSignals).unwrap());

    assert!(
        report.contains(
            "Console (stderr), since the last call:\n\n```\nreentrant operation in its \
             NSTableView delegate\n```"
        ),
        "unexpected report: {report}"
    );
}

/// Named relative to the repository, not absolutely.
/// A report is meant to be pasteable into an issue, and an absolute path here
/// says whose machine produced it.
#[test]
fn names_what_survives_for_a_relaunch() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let session = record(root, DEAD_PID);

    let report = content(run(root, &dir_for(root), GRACE, &RealSignals).unwrap());

    assert!(
        report.contains(
            "Kept for a relaunch with `fresh = false`:\n\n- state: `tmp/debug-app/test/state`\n- \
             user data: `tmp/debug-app/test/data`\n"
        ),
        "unexpected report: {report}"
    );
    assert!(
        !report.contains(root.as_str()),
        "the report names the machine: {report}"
    );
    assert!(session.state_dir.is_dir());
}
