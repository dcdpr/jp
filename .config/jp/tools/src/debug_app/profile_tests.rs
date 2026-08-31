use std::{fs, sync::Mutex, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};

use super::{start, stop};
use crate::{
    Error,
    debug_app::{
        capture::{Recording, Scope, Spawner, Tier, pending, unix_seconds},
        session::{Console, RealSignals, Session, Signal, Signals, Slot},
    },
    util::runner::MockProcessRunner,
};

/// A recorder that is already gone, so a stop resolves without waiting.
struct Gone;

impl Signals for Gone {
    fn send(&self, _pid: u32, _signal: Signal) {}

    fn is_alive(&self, _pid: u32) -> bool {
        false
    }
}

/// What `codesign -d --entitlements -` prints for a bundle that can be attached
/// to.
const ENTITLEMENTS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>com.apple.security.get-task-allow</key><true/></dict></plist>"#;

/// A [`Spawner`] that records the command line and hands back a pid.
struct FakeSpawner {
    started: Mutex<Vec<Vec<String>>>,
    pid: u32,
}

impl FakeSpawner {
    fn returning(pid: u32) -> Self {
        Self {
            started: Mutex::new(Vec::new()),
            pid,
        }
    }

    fn started(&self) -> Vec<Vec<String>> {
        self.started.lock().unwrap().clone()
    }
}

impl Spawner for FakeSpawner {
    fn start(
        &self,
        args: &[String],
        log: &Utf8Path,
        _working_dir: &Utf8Path,
        _timeout: Duration,
    ) -> Result<u32, Error> {
        self.started.lock().unwrap().push(args.to_vec());

        // The real one leaves what the recorder said here, and `stop` reads it
        // back looking for run issues.
        if let Some(parent) = log.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(log, "Ctrl-C to stop the recording\n").unwrap();

        Ok(self.pid)
    }
}

/// A [`Spawner`] that fails the test if a recorder is started.
struct NeverSpawns;

impl Spawner for NeverSpawns {
    fn start(
        &self,
        args: &[String],
        _log: &Utf8Path,
        _working_dir: &Utf8Path,
        _timeout: Duration,
    ) -> Result<u32, Error> {
        panic!("the recorder was started: {args:?}");
    }
}

fn temp() -> (camino_tempfile::Utf8TempDir, Utf8PathBuf) {
    let workspace = camino_tempfile::tempdir().unwrap();
    let dir = Session::dir(workspace.path(), &Slot::fixed("test"));

    (workspace, dir)
}

/// Record a running session, including the pid file the app writes.
fn running_session(dir: &Utf8Path, allocation_stacks: bool) -> Session {
    let pid = std::process::id();
    let session = Session {
        pid,
        bundle: "/derived/JP.app".into(),
        configuration: "Debug".to_owned(),
        workspace: "/repo".into(),
        state_dir: dir.join("state"),
        user_data_dir: dir.join("data"),
        stdout: Console::new(dir.join("console.out")),
        stderr: Console::new(dir.join("console.err")),
        trace: Console::new(dir.join("state/trace.jsonl")),
        reported_footprint_mb: None,
        dsym: None,
        allocation_stacks,
    };

    session.store(dir).unwrap();
    fs::create_dir_all(&session.state_dir).unwrap();
    fs::write(session.pid_path(), format!("{pid}\n")).unwrap();
    session
}

fn content(outcome: jp_tool::Outcome) -> String {
    match outcome {
        jp_tool::Outcome::Success { content } => content,
        jp_tool::Outcome::Error { message, .. } => message,
        other @ jp_tool::Outcome::NeedsInput { .. } => panic!("unexpected outcome: {other:?}"),
    }
}

/// With an app running there is a process to attach to, and the trace holds it
/// alone.
/// Nothing tells the tool which to use — the ordering does.
#[test]
fn starting_against_a_running_app_attaches_to_it() {
    let (workspace, dir) = temp();
    let session = running_session(&dir, false);
    let spawner = FakeSpawner::returning(4321);
    let runner = MockProcessRunner::builder()
        .expect("codesign")
        .returns_success(ENTITLEMENTS);

    let report =
        content(start(workspace.path(), &dir, &[Tier::Sampling], &runner, &spawner).unwrap());

    let started = spawner.started();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(
        started[0].contains(&"--attach".to_owned())
            && started[0].contains(&session.pid.to_string()),
        "expected an attach: {:?}",
        started[0]
    );
    assert!(
        !started[0].contains(&"--all-processes".to_owned()),
        "{:?}",
        started[0]
    );

    assert!(
        report.contains(&format!("Attached to the app (pid {})", session.pid)),
        "{report}"
    );

    let open = pending(&dir).expect("the bracket should be recorded");
    assert_eq!(open.scope, Scope::Attach(session.pid));
    assert_eq!(open.recorder_pid, 4321);
}

/// With no app there is nothing to attach to, so the recorder takes the
/// machine.
/// That is the only way to cover a launch, and the report has to say what it
/// will cost.
#[test]
fn starting_with_no_app_records_the_machine_and_says_what_that_costs() {
    let (workspace, dir) = temp();
    let spawner = FakeSpawner::returning(4321);

    // Nothing to check the entitlement of, so codesign is never run.
    let runner = MockProcessRunner::never_called();

    let report =
        content(start(workspace.path(), &dir, &[Tier::Sampling], &runner, &spawner).unwrap());

    let started = spawner.started();
    assert!(
        started[0].contains(&"--all-processes".to_owned()),
        "{:?}",
        started[0]
    );
    assert!(
        !started[0].contains(&"--attach".to_owned()),
        "{:?}",
        started[0]
    );

    assert!(report.contains("every process on the machine"), "{report}");
    assert!(report.contains("will take minutes"), "{report}");

    assert_eq!(pending(&dir).map(|r| r.scope), Some(Scope::System));
}

/// libmalloc reads `MallocStackLogging` at process start, so an app launched
/// without it kept no stacks and the instrument would find nothing.
/// Refusing and naming the relaunch beats recording an empty table.
#[test]
fn allocations_against_an_app_launched_without_them_is_refused() {
    let (workspace, dir) = temp();
    let session = running_session(&dir, false);
    let runner = MockProcessRunner::never_called();

    let report = content(
        start(
            workspace.path(),
            &dir,
            &[Tier::Sampling, Tier::Allocations],
            &runner,
            &NeverSpawns,
        )
        .unwrap(),
    );

    assert!(
        report.contains(&format!(
            "The app running as pid {} was not launched with `allocation_stacks`",
            session.pid
        )),
        "unexpected report: {report}"
    );
    assert!(
        report.contains("`debug_app_launch` again with `allocation_stacks: true`"),
        "{report}"
    );
    assert_eq!(pending(&dir), None);
}

/// `xctrace` reports `Allocations cannot handle a target type of 'All
/// Processes'` and then fails the whole recording, so the combination has to be
/// refused before a recorder is ever started.
#[test]
fn allocations_with_no_app_running_is_refused() {
    let (workspace, dir) = temp();
    let runner = MockProcessRunner::never_called();

    let report = content(
        start(
            workspace.path(),
            &dir,
            &[Tier::Sampling, Tier::Allocations],
            &runner,
            &NeverSpawns,
        )
        .unwrap(),
    );

    assert!(
        report.starts_with("Allocations cannot be recorded with no app running."),
        "unexpected report: {report}"
    );
    assert!(
        report.contains("`debug_app_launch` with `allocation_stacks: true`"),
        "{report}"
    );
    assert_eq!(pending(&dir), None);
}

/// The one ordering that works: an app launched to keep allocation stacks, then
/// a bracket attached to it.
#[test]
fn allocations_are_accepted_against_an_app_launched_for_them() {
    let (workspace, dir) = temp();
    let session = running_session(&dir, true);
    let spawner = FakeSpawner::returning(4321);
    let runner = MockProcessRunner::builder()
        .expect("codesign")
        .returns_success(ENTITLEMENTS);

    start(
        workspace.path(),
        &dir,
        &[Tier::Sampling, Tier::Allocations],
        &runner,
        &spawner,
    )
    .unwrap();

    let open = pending(&dir).expect("the bracket should be recorded");
    assert!(open.holds(Tier::Allocations));
    assert_eq!(open.scope, Scope::Attach(session.pid));

    let started = &spawner.started()[0];
    assert!(started.contains(&"Allocations".to_owned()), "{started:?}");
    assert!(
        !started.contains(&"--all-processes".to_owned()),
        "the instrument refuses that target: {started:?}"
    );
}

#[test]
fn starting_a_second_bracket_is_refused() {
    let (workspace, dir) = temp();
    let open = Recording {
        id: "profile-1".to_owned(),
        tiers: vec![Tier::Sampling],
        scope: Scope::System,
        recorder_pid: std::process::id(),
        started_unix: unix_seconds(),
        stopped_unix: None,
        target: None,
    };
    open.store(&dir).unwrap();

    let runner = MockProcessRunner::never_called();
    let report = content(
        start(
            workspace.path(),
            &dir,
            &[Tier::Sampling],
            &runner,
            &NeverSpawns,
        )
        .unwrap(),
    );

    assert!(
        report.starts_with("A recording is already open as `profile-1`"),
        "unexpected report: {report}"
    );
}

/// An app whose ad-hoc re-sign dropped the entitlement cannot be attached to,
/// and the recorder would produce nothing.
#[test]
fn starting_against_an_app_that_cannot_be_attached_to_fails() {
    let (workspace, dir) = temp();
    running_session(&dir, false);
    let runner = MockProcessRunner::builder()
        .expect("codesign")
        .returns_success("<plist version=\"1.0\"><dict/></plist>");

    let error = start(
        workspace.path(),
        &dir,
        &[Tier::Sampling],
        &runner,
        &NeverSpawns,
    )
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("does not carry `get-task-allow`"),
        "unexpected error: {error}"
    );
}

#[test]
fn stopping_with_no_bracket_open_says_so() {
    let (workspace, dir) = temp();

    let report = content(stop(workspace.path(), &dir, false, &RealSignals).unwrap());

    assert!(
        report.starts_with("No recording is open in"),
        "unexpected report: {report}"
    );
}

/// Reading a system-wide recording costs minutes, so throwing a botched bracket
/// away has to be possible without paying for it.
#[test]
fn discarding_a_bracket_deletes_the_bundle_unread() {
    let (workspace, dir) = temp();
    let open = Recording {
        id: "profile-1".to_owned(),
        tiers: vec![Tier::Sampling],
        scope: Scope::System,
        recorder_pid: std::process::id(),
        started_unix: unix_seconds(),
        stopped_unix: None,
        target: None,
    };
    fs::create_dir_all(open.bundle(&dir)).unwrap();
    fs::write(open.log(&dir), "Ctrl-C to stop the recording\n").unwrap();
    open.store(&dir).unwrap();

    let report = content(stop(workspace.path(), &dir, true, &Gone).unwrap());

    assert!(
        report.starts_with("Discarded `profile-1`"),
        "unexpected report: {report}"
    );
    assert!(report.contains("unread"), "{report}");
    assert!(!open.bundle(&dir).exists());
    assert_eq!(pending(&dir), None);
}
