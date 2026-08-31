use std::fs;

use camino::{Utf8Path, Utf8PathBuf};

use super::{NO_SCREEN_RECORDING, Window, WindowList, no_window, parse, report, run, shot_path};
use crate::{
    debug_app::session::{Console, Session, Slot},
    util::runner::MockProcessRunner,
};

/// A slot every test in this file shares, so paths are predictable.
fn dir_for(root: &Utf8Path) -> Utf8PathBuf {
    Session::dir(root, &Slot::fixed("test"))
}

fn session() -> Session {
    let dir = dir_for(Utf8Path::new("/repo"));
    Session {
        pid: 4321,
        bundle: "/derived/JP.app".into(),
        configuration: "Debug".to_owned(),
        workspace: "/repo/tmp/debug-app/test/workspace".into(),
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

fn window(title: Option<&str>) -> Window {
    Window {
        id: 7412,
        title: title.map(ToOwned::to_owned),
        width: 1200,
        height: 800,
    }
}

/// Record a live session, and stage a file where `driver::locate` looks for the
/// driver binary.
///
/// The recorded pid is this process: `Session::resolve` refuses a pid that is
/// not running, so a fabricated one would fail before reaching the branch under
/// test.
fn record(root: &Utf8Path) -> Utf8PathBuf {
    let dir = dir_for(root);
    let session = Session {
        pid: std::process::id(),
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
    fs::write(session.pid_path(), format!("{}\n", session.pid)).unwrap();

    let bin_dir = root.join("driver-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(bin_dir.join("jpdrive"), "").unwrap();
    bin_dir
}

/// A runner that answers the two build lookups [`driver::locate`] makes, and
/// then the window list.
fn runner(bin_dir: &Utf8Path, windows: &str) -> MockProcessRunner {
    MockProcessRunner::builder()
        .expect("just")
        .args(&["build-drive"])
        .returns_success("")
        .expect("swift")
        .returns_success(format!("{bin_dir}\n"))
        .expect(bin_dir.join("jpdrive").as_str())
        .returns_success(windows)
}

fn content(outcome: jp_tool::Outcome) -> String {
    match outcome {
        jp_tool::Outcome::Success { content } => content,
        jp_tool::Outcome::Error { message, .. } => message,
        other @ jp_tool::Outcome::NeedsInput { .. } => panic!("unexpected outcome: {other:?}"),
    }
}

/// Without the grant the window server hands back the desktop, which looks like
/// a successful capture of the wrong thing.
/// Nothing may be written on that path, and `screencapture` must never run.
#[test]
fn refuses_without_the_screen_recording_grant() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let bin_dir = record(root);
    let listed = r#"{"screen_recording":false,"windows":[{"id":7412,"title":"JP","width":1200,"height":800}]}"#;

    let outcome = run(
        root,
        &dir_for(root),
        1_730_000_000_123,
        &runner(&bin_dir, listed),
    )
    .unwrap();

    assert_eq!(content(outcome), NO_SCREEN_RECORDING);
    assert_eq!(
        fs::read_dir(dir_for(root))
            .unwrap()
            .filter_map(|entry| Some(entry.ok()?.file_name().to_str()?.to_owned()))
            .filter(|name| name.starts_with("shot-"))
            .count(),
        0,
        "a refused capture must leave no file behind"
    );
}

#[test]
fn reports_an_app_with_no_window_on_screen() {
    let workspace = camino_tempfile::tempdir().unwrap();
    let root = workspace.path();
    let bin_dir = record(root);
    let listed = r#"{"screen_recording":true,"windows":[]}"#;

    let outcome = run(
        root,
        &dir_for(root),
        1_730_000_000_123,
        &runner(&bin_dir, listed),
    )
    .unwrap();

    assert_eq!(
        content(outcome),
        format!(
            "The app (pid {}) has no window at all, so there is nothing to capture. A window that \
             is minimized or closed is absent from the window server's list entirely.",
            std::process::id()
        )
    );
}

/// A window on another desktop is missing from the on-screen list and from the
/// accessibility tree alike, which reads exactly like an app that never opened
/// one.
/// Telling the two apart is the difference between switching desktop and
/// hunting a bug that is not there — which is what happened before this
/// existed.
#[test]
fn distinguishes_a_window_on_another_space_from_no_window() {
    let elsewhere = WindowList {
        screen_recording: true,
        windows: vec![],
        other_spaces: vec![window(Some("JP"))],
    };

    assert_eq!(
        no_window(4321, &elsewhere, "capture"),
        "The app (pid 4321) has 1 window(s), all of them on another Space, so there is nothing to \
         capture and the accessibility tree reports none either. Switch to the desktop the app is \
         on, or move its window to this one."
    );
}

/// The verb is the caller's, so one message serves the capture and the scan.
#[test]
fn names_what_the_caller_was_trying_to_do() {
    let empty = WindowList {
        screen_recording: true,
        windows: vec![],
        other_spaces: vec![],
    };

    assert!(no_window(1, &empty, "read").contains("nothing to read"));
}

/// The exact document `jpdrive windowid` writes.
/// Nothing else checks that the two sides agree on the key names, and a
/// mismatch reads as an app with no windows rather than as a parse failure.
#[test]
fn reads_the_document_the_driver_writes() {
    let listed = r#"{
      "screen_recording" : true,
      "windows" : [
        {
          "height" : 800,
          "id" : 7412,
          "title" : "JP",
          "width" : 1200
        }
      ]
    }"#;

    let list = parse(listed).unwrap();

    assert!(list.screen_recording);
    assert_eq!(list.windows.len(), 1);
    assert_eq!(list.windows[0].id, 7412);
    assert_eq!(list.windows[0].title.as_deref(), Some("JP"));
    assert_eq!(list.windows[0].width, 1200);
    assert_eq!(list.windows[0].height, 800);
}

#[test]
fn rejects_a_window_list_it_cannot_read() {
    let error = parse("not json").unwrap_err().to_string();

    assert!(
        error.starts_with("Failed to parse the window list `jpdrive` reported:"),
        "unexpected error: {error}"
    );
}

#[test]
fn reports_where_the_capture_landed_and_how_to_attach_it() {
    let report = report(
        Utf8Path::new("/repo"),
        &session(),
        &window(Some("JP - jp")),
        1,
        Utf8Path::new("/repo/tmp/debug-app/test/shot-1730000000123.png"),
        188_416,
    );

    assert_eq!(
        report,
        "Captured window 7412 \"JP - jp\" of the app (pid 4321), 1200x800 points.\n\nWritten to \
         `tmp/debug-app/test/shot-1730000000123.png` (184 KiB).\n\nA tool result is text, so the \
         image does not reach the assistant from here. Attach the file on the next turn to have \
         it looked at:\n\n```sh\njp query -a tmp/debug-app/test/shot-1730000000123.png \"what is \
         wrong with this layout?\"\n```\n"
    );
}

/// A second window is the case where the capture answers a question about the
/// wrong one, so the report says which it took.
#[test]
fn says_when_the_app_has_more_than_one_window() {
    let report = report(
        Utf8Path::new("/repo"),
        &session(),
        &window(Some("JP - jp")),
        3,
        Utf8Path::new("/repo/tmp/debug-app/test/shot-1730000000123.png"),
        188_416,
    );

    assert!(
        report.contains("\nThe app has 3 windows on screen. This is the frontmost one.\n"),
        "unexpected report: {report}"
    );
}

#[test]
fn names_an_untitled_window_by_its_number_alone() {
    let report = report(
        Utf8Path::new("/repo"),
        &session(),
        &window(None),
        1,
        Utf8Path::new("/repo/tmp/debug-app/test/shot-1730000000123.png"),
        188_416,
    );

    assert!(
        report.starts_with("Captured window 7412 of the app (pid 4321), 1200x800 points.\n"),
        "unexpected report: {report}"
    );
}

/// Two shots taken in one session are compared against each other, so the
/// second must not erase the first.
#[test]
fn names_each_capture_by_when_it_was_taken() {
    let dir = Utf8Path::new("/repo/tmp/debug-app/test");

    assert_eq!(
        shot_path(dir, 1_730_000_000_123),
        "/repo/tmp/debug-app/test/shot-1730000000123.png"
    );
    assert_ne!(shot_path(dir, 1), shot_path(dir, 2));
}
