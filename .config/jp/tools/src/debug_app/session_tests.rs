use std::fs;

use camino::Utf8Path;

use super::{Console, Session, Slot, pid_is_alive};

/// A slot every test in this file shares, so paths are predictable.
fn slot() -> Slot {
    Slot::fixed("test")
}

/// Above macOS's default maximum pid, so no process can hold it.
const DEAD_PID: u32 = 4_000_000;

/// A session whose state directory is inside `root`.
fn session(root: &Utf8Path, pid: u32) -> Session {
    let dir = Session::dir(root, &slot());
    Session {
        pid,
        bundle: Utf8Path::new("/tmp/JP.app").to_owned(),
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
    }
}

/// A record written before the trace stream existed.
///
/// Kept as literal JSON rather than built from the current struct, which would
/// pass whatever fields that struct happens to have and prove nothing.
const OLD_RECORD: &str = r#"{
  "pid": 4321,
  "bundle": "/tmp/JP.app",
  "configuration": "Debug",
  "workspace": "/repo/workspace",
  "state_dir": "/repo/tmp/debug-app/test/state",
  "user_data_dir": "/repo/tmp/debug-app/test/data",
  "stdout": { "path": "/repo/tmp/debug-app/test/console.out", "offset": 12 },
  "stderr": { "path": "/repo/tmp/debug-app/test/console.err", "offset": 0 }
}"#;

/// A tool that refused to load a record written by the previous build would
/// strand a running app: nothing can address it, and nothing can stop it.
#[test]
fn a_record_without_a_trace_stream_still_loads() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let dir = Session::dir(workspace.path(), &slot());
    fs::create_dir_all(&dir).unwrap();
    fs::write(Session::path(&dir), OLD_RECORD).unwrap();

    let mut loaded = Session::load(&dir).unwrap().unwrap();

    assert_eq!(loaded.pid, 4321);
    assert_eq!(loaded.reported_footprint_mb, None);
    assert_eq!(loaded.trace.path, "");
    // The stream reads as empty rather than as an error, so a snapshot of that
    // app reports its tree and console as it always did.
    assert_eq!(loaded.trace.delta().unwrap(), "");
}

/// Write the pid file the app is responsible for.
fn write_pid(session: &Session, pid: u32) {
    fs::create_dir_all(&session.state_dir).unwrap();
    fs::write(session.pid_path(), format!("{pid}\n")).unwrap();
}

/// The conversation is what makes two agents land in different slots without
/// either of them choosing to.
#[test]
fn slot_comes_from_the_conversation() {
    assert_eq!(
        Slot::named(None, "jp-c12345").unwrap().as_str(),
        "jp-c12345"
    );
    assert_ne!(
        Slot::named(None, "jp-c12345").unwrap(),
        Slot::named(None, "jp-c67890").unwrap()
    );
}

/// Two conversations sharing one running instance is a deliberate act, so it
/// takes an explicit name.
#[test]
fn an_override_wins_over_the_conversation() {
    assert_eq!(
        Slot::named(Some("shared"), "jp-c12345").unwrap().as_str(),
        "shared"
    );
}

/// A conversation id is nobody's choice, so what a bundle identifier cannot
/// take is dropped from it rather than reported.
#[test]
fn a_conversation_id_is_reduced_to_what_an_identifier_accepts() {
    assert_eq!(Slot::named(None, "jp/c/123").unwrap().as_str(), "jpc123");
}

/// An override is somebody's choice, and filtering it answers a different
/// question than the one asked: `my slot` would run under `myslot`, whose
/// artifacts are not where the caller goes looking.
///
/// Two names that differ only in what a filter removes would also collapse into
/// one, so two agents deliberately kept apart would drive the same app.
#[test]
fn an_override_naming_something_unusable_is_refused() {
    let error = Slot::named(Some("agent_two/../etc"), "jp-c12345")
        .unwrap_err()
        .to_string();

    assert!(
        error.starts_with("`JP_DEBUG_APP_SLOT` is \"agent_two/../etc\", which cannot name a slot"),
        "unexpected error: {error}"
    );

    assert!(Slot::named(Some("///"), "jp-c12345").is_err());
    assert!(Slot::named(Some("my slot"), "jp-c12345").is_err());
}

/// Two names a filter would have reduced to one stay distinct, because neither
/// is accepted in the first place.
#[test]
fn two_overrides_a_filter_would_merge_are_both_refused() {
    assert!(Slot::named(Some("agent_one"), "").is_err());
    assert_eq!(
        Slot::named(Some("agentone"), "").unwrap().as_str(),
        "agentone"
    );
}

/// An unset variable is not a name, so it defers to the conversation, and a
/// conversation with nothing usable falls back rather than failing.
#[test]
fn slot_falls_through_then_back() {
    assert_eq!(
        Slot::named(Some(""), "jp-c12345").unwrap().as_str(),
        "jp-c12345"
    );
    assert_eq!(Slot::named(None, "").unwrap().as_str(), "default");
    assert_eq!(Slot::named(Some(""), "///").unwrap().as_str(), "default");
}

#[test]
fn load_returns_none_without_a_record() {
    let workspace = camino_tempfile::tempdir().unwrap();

    assert!(
        Session::load(&Session::dir(workspace.path(), &slot()))
            .unwrap()
            .is_none()
    );
}

#[test]
fn store_then_load_round_trips() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let stored = session(root, 4321);

    stored.store(&Session::dir(root, &slot())).unwrap();
    let loaded = Session::load(&Session::dir(root, &slot()))
        .unwrap()
        .unwrap();

    assert_eq!(loaded.pid, 4321);
    assert_eq!(loaded.configuration, "Debug");
    assert_eq!(loaded.workspace, stored.workspace);
    assert_eq!(loaded.stdout.path, stored.stdout.path);
    assert_eq!(loaded.stdout.offset, 0);
}

#[test]
fn resolve_without_a_record_names_the_launch_tool() {
    let workspace = camino_tempfile::tempdir().unwrap();

    let error = Session::resolve(&Session::dir(workspace.path(), &slot()))
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        format!(
            "No app session recorded at {}. Start one with `debug_app_launch`.",
            Session::path(&Session::dir(workspace.path(), &slot()))
        )
    );
}

#[test]
fn resolve_without_a_pid_file_reports_an_unconfirmable_session() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let stored = session(root, 4321);
    stored.store(&Session::dir(root, &slot())).unwrap();

    let error = Session::resolve(&Session::dir(root, &slot()))
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        format!(
            "The app's pid file at {} is gone, so the recorded session (pid 4321) can no longer \
             be confirmed. The app was most likely quit outside these tools. Run \
             `debug_app_launch` to start a new one.",
            stored.pid_path()
        )
    );
}

/// The case the session record exists to catch: an instance nobody here
/// started.
#[test]
fn resolve_with_a_different_pid_reports_the_mismatch() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let stored = session(root, 4321);
    stored.store(&Session::dir(root, &slot())).unwrap();
    write_pid(&stored, 9876);

    let error = Session::resolve(&Session::dir(root, &slot()))
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        format!(
            "The app running under {} reports pid 9876, but the recorded session is pid 4321. \
             Something launched the app outside these tools. Run `debug_app_quit` and then \
             `debug_app_launch` to get back to a known state.",
            stored.state_dir
        )
    );
}

/// Killing the app out from under the tools has to say so, and say what the app
/// complained about on the way out, rather than hang or report an empty
/// snapshot.
#[test]
fn resolve_with_a_dead_process_quotes_the_last_stderr() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let stored = session(root, DEAD_PID);
    stored.store(&Session::dir(root, &slot())).unwrap();
    write_pid(&stored, DEAD_PID);
    fs::write(
        &stored.stderr.path,
        "*** Assertion failure in -[NSTableView ...]\n",
    )
    .unwrap();

    let error = Session::resolve(&Session::dir(root, &slot()))
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        "The app recorded as pid 4000000 is no longer running — it quit or crashed. Its last \
         stderr output was:\n\n```\n*** Assertion failure in -[NSTableView ...]\n```\n\nRun \
         `debug_app_launch` to start a new one."
    );
}

#[test]
fn resolve_with_a_dead_and_silent_process_says_so() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let stored = session(root, DEAD_PID);
    stored.store(&Session::dir(root, &slot())).unwrap();
    write_pid(&stored, DEAD_PID);

    let error = Session::resolve(&Session::dir(root, &slot()))
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        "The app recorded as pid 4000000 is no longer running — it quit or crashed. It wrote \
         nothing to stderr before going away.\n\nRun `debug_app_launch` to start a new one."
    );
}

#[test]
fn resolve_accepts_a_live_matching_process() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let pid = std::process::id();
    let stored = session(root, pid);
    stored.store(&Session::dir(root, &slot())).unwrap();
    write_pid(&stored, pid);

    let resolved = Session::resolve(&Session::dir(root, &slot())).unwrap();

    assert_eq!(resolved.pid, pid);
}

#[test]
fn is_running_is_false_for_a_dead_process() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let stored = session(root, DEAD_PID);
    write_pid(&stored, DEAD_PID);

    assert!(!stored.is_running());
}

#[test]
fn pid_is_alive_rejects_a_pid_no_process_can_hold() {
    assert!(!pid_is_alive(DEAD_PID));
    assert!(pid_is_alive(std::process::id()));
}

#[test]
fn delta_returns_only_what_was_appended() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let path = workspace.path().join("console.err");
    fs::write(&path, "first\n").unwrap();

    let mut console = Console::new(path.clone());
    assert_eq!(console.delta().unwrap(), "first\n");
    assert_eq!(console.offset, 6);

    fs::write(&path, "first\nsecond\n").unwrap();
    assert_eq!(console.delta().unwrap(), "second\n");
    assert_eq!(console.offset, 13);

    assert_eq!(console.delta().unwrap(), "");
}

/// A relaunch truncates the console files, which would otherwise leave the
/// offset past the end of the file and report nothing forever.
#[test]
fn delta_returns_everything_after_a_truncation() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let path = workspace.path().join("console.err");
    fs::write(&path, "a long first run\n").unwrap();

    let mut console = Console::new(path.clone());
    console.delta().unwrap();

    fs::write(&path, "short\n").unwrap();

    assert_eq!(console.delta().unwrap(), "short\n");
}

#[test]
fn delta_of_a_missing_file_is_empty() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let mut console = Console::new(workspace.path().join("nothing-here"));

    assert_eq!(console.delta().unwrap(), "");
    assert_eq!(console.offset, 0);
}

#[test]
fn tail_ignores_the_offset() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let path = workspace.path().join("console.err");
    fs::write(&path, "already reported\n").unwrap();

    let mut console = Console::new(path);
    console.delta().unwrap();

    assert_eq!(console.tail(), "already reported\n");
}

#[test]
fn tail_of_a_missing_file_is_empty() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let console = Console::new(workspace.path().join("nothing-here"));

    assert_eq!(console.tail(), "");
}
