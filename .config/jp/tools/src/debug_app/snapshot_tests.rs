use camino::Utf8Path;

use super::{format_preview, report};
use crate::{
    debug_app::{
        session::{Console, Session},
        tree::Options,
    },
    util::paths::{Shortening, shortenings_from},
};

/// A repository at `/repo`, so the fixture's paths shorten to relative ones.
fn shortenings() -> Vec<Shortening> {
    shortenings_from(Utf8Path::new("/repo"), Some("/Users/jean"), None, None)
}

fn session() -> Session {
    Session {
        pid: 4321,
        bundle: Utf8Path::new("/tmp/JP.app").to_owned(),
        configuration: "Debug".to_owned(),
        workspace: Utf8Path::new("/repo/tmp/debug-app/workspace").to_owned(),
        state_dir: Utf8Path::new("/repo/tmp/debug-app/state").to_owned(),
        user_data_dir: Utf8Path::new("/repo/tmp/debug-app/data").to_owned(),
        stdout: Console::new(Utf8Path::new("/repo/tmp/debug-app/console.out").to_owned()),
        stderr: Console::new(Utf8Path::new("/repo/tmp/debug-app/console.err").to_owned()),
        trace: Console::new(Utf8Path::new("/repo/tmp/debug-app/state/trace.jsonl").to_owned()),
        reported_footprint_mb: None,
        dsym: None,
        allocation_stacks: false,
    }
}

#[test]
fn reports_the_tree_and_both_console_streams() {
    let report = report(
        &session(),
        "AXApplication\n  AXWindow \"mac-app\"\n",
        None,
        "opened workspace\n",
        "*** constraint complaint\n",
        None,
        &shortenings(),
    );

    assert_eq!(
        report,
        "Snapshot of the app (pid 4321) on `tmp/debug-app/workspace`.\n\nAccessibility \
         tree:\n\n```\nAXApplication\n  AXWindow \"mac-app\"\n```\n\nConsole (stdout), since the \
         last call:\n\n```\nopened workspace\n```\n\nConsole (stderr), since the last \
         call:\n\n```\n*** constraint complaint\n```\n"
    );
}

/// An empty console section would read as "the app said nothing at all", when
/// what it means is "nothing since the last call".
#[test]
fn says_when_neither_stream_has_anything_new() {
    let report = report(
        &session(),
        "AXApplication\n",
        None,
        "",
        "  \n",
        None,
        &shortenings(),
    );

    assert_eq!(
        report,
        "Snapshot of the app (pid 4321) on `tmp/debug-app/workspace`.\n\nAccessibility \
         tree:\n\n```\nAXApplication\n```\n\nNothing new on either console stream since the last \
         call.\n"
    );
}

/// The trace block sits last, after whatever the app said on its console: it
/// summarizes the same work those lines came from.
#[test]
fn reports_the_trace_summary_after_the_console() {
    let report = report(
        &session(),
        "AXApplication\n",
        None,
        "",
        "",
        Some(
            "Trace, since the last call: 1 span. Slowest `transcript.render` 84 ms.\nFootprint \
             412 MB (+38 MB).",
        ),
        &shortenings(),
    );

    assert_eq!(
        report,
        "Snapshot of the app (pid 4321) on `tmp/debug-app/workspace`.\n\nAccessibility \
         tree:\n\n```\nAXApplication\n```\n\nNothing new on either console stream since the last \
         call.\n\nTrace, since the last call: 1 span. Slowest `transcript.render` 84 \
         ms.\nFootprint 412 MB (+38 MB).\n"
    );
}

/// The preview is what a caller reads before approving the call, so it has to
/// name the command that will run against the app.
#[test]
fn the_preview_names_the_read_it_will_perform() {
    let opts = Options {
        identifier: Some("sidebar.".to_owned()),
        max_matches: Some(1),
        ..Options::default()
    };

    assert!(format_preview(&opts, false).contains(
        "jpdrive tree --pid <pid> --max-siblings 0 --identifier sidebar. --max-matches 1"
    ));
}

/// The pasteboard belongs to the system rather than to the app, so a report
/// that quoted it unasked would leak whatever the user last copied.
#[test]
fn quotes_the_pasteboard_only_when_it_was_asked_for() {
    let quoted = report(
        &session(),
        "AXApplication\n",
        Some("jp://jp-c12345\njp://jp-c67890\n"),
        "",
        "",
        None,
        &shortenings(),
    );
    assert!(
        quoted.contains("\nPasteboard:\n\n```\njp://jp-c12345\njp://jp-c67890\n```\n"),
        "unexpected report: {quoted}"
    );

    let unasked = report(
        &session(),
        "AXApplication\n",
        None,
        "",
        "",
        None,
        &shortenings(),
    );
    assert!(
        !unasked.contains("Pasteboard"),
        "unexpected report: {unasked}"
    );
}

/// An empty clipboard is an answer — a Copy Link that did nothing looks
/// exactly like this — so it is reported rather than left out.
#[test]
fn says_when_the_pasteboard_is_empty() {
    let report = report(
        &session(),
        "AXApplication\n",
        Some(""),
        "",
        "",
        None,
        &shortenings(),
    );

    assert!(
        report.contains("\nThe pasteboard is empty.\n"),
        "unexpected report: {report}"
    );
}

#[test]
fn the_preview_names_the_pasteboard_read_when_asked() {
    let preview = format_preview(&Options::default(), true);

    assert!(
        preview.contains("\npbpaste\n"),
        "unexpected preview: {preview}"
    );
    assert!(!format_preview(&Options::default(), false).contains("pbpaste"));
}
