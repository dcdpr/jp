use std::io;

use camino::Utf8Path;
use test_log::test;

use super::*;

/// `ENOSPC` on every Unix platform this crate targets.
///
/// These tests assert the exact text the user sees, which includes the OS's own
/// message, so they build the error from the raw code the kernel delivers
/// rather than naming [`io::ErrorKind::StorageFull`] directly.
#[cfg(unix)]
const ENOSPC: i32 = 28;

/// Render a CLI error the way the terminal output does: the message, then each
/// metadata entry as `key=value`.
fn rendered(error: &Error) -> String {
    let mut lines = vec![error.message.clone().unwrap_or_default()];
    for (key, value) in &error.metadata {
        lines.push(format!("{key}={}", value.as_str().unwrap_or_default()));
    }
    lines.join("\n")
}

/// A drop-time persist failure as the workspace records one.
fn persist_failure() -> jp_workspace::Error {
    jp_workspace::Error::Storage(jp_storage::Error::write_failed(
        Utf8Path::new("/data/conv/events.json"),
        io::Error::from(io::ErrorKind::StorageFull),
    ))
}

#[test]
fn a_clean_run_passes_through_unchanged() {
    assert!(fold_persist_failure(Ok(()), None).is_ok());
}

#[test]
fn a_persist_failure_fails_an_otherwise_successful_run() {
    let error =
        fold_persist_failure(Ok(()), Some(persist_failure())).expect_err("must not exit zero");

    assert_eq!(error.message.as_deref(), Some("No space left on device"));
    assert_eq!(error.code.get(), 1);
}

#[test]
fn a_persist_failure_rides_along_with_an_existing_error() {
    // The two can be independent — a provider error and a full disk — so the
    // primary error stays the headline and the persist failure is attached
    // rather than dropped.
    let error = fold_persist_failure(Err(Error::from("Rate limited")), Some(persist_failure()))
        .expect_err("the primary error is preserved");

    assert_eq!(error.message.as_deref(), Some("Rate limited"));
    assert_eq!(
        error
            .metadata
            .iter()
            .find(|(key, _)| key == "persist_failure")
            .map(|(_, value)| value.as_str().unwrap_or_default()),
        Some("Storage error: no space left on device while writing /data/conv/events.json")
    );
}

#[test]
fn a_persist_failure_makes_an_expected_outcome_a_failure() {
    // `jp conversation grep` exits 1 with `expected` set when it finds nothing,
    // which suppresses the trace-log announcement. A run that could not save is
    // broken regardless of how it was going to exit, so the flag has to clear —
    // otherwise a data-loss run hides the log the user needs to diagnose it.
    let error = fold_persist_failure(
        Err(Error::from("No matches").expected()),
        Some(persist_failure()),
    )
    .expect_err("the primary error is preserved");

    assert!(!error.expected);
}

#[test]
fn an_expected_outcome_stays_expected_when_nothing_was_recorded() {
    let error = fold_persist_failure(Err(Error::from("No matches").expected()), None)
        .expect_err("the primary error is preserved");

    assert!(error.expected);
}

#[test]
fn an_existing_error_survives_when_nothing_was_recorded() {
    let error = fold_persist_failure(Err(Error::from("Rate limited")), None)
        .expect_err("the primary error is preserved");

    assert_eq!(error.message.as_deref(), Some("Rate limited"));
    assert!(error.metadata.is_empty());
}

#[cfg(unix)]
#[test]
fn out_of_space_renders_path_cause_and_action() {
    let error = Error::from(jp_storage::Error::write_failed(
        Utf8Path::new("/data/conv/events.json"),
        io::Error::from_raw_os_error(ENOSPC),
    ));

    assert_eq!(
        rendered(&error),
        "No space left on device\npath=/data/conv/events.json\nerror=No space left on device (os \
         error 28)\nsuggestion=Free up disk space and re-run. Anything after the last successful \
         write was not saved."
    );
    assert_eq!(error.code.get(), 1);
}

#[test]
fn write_failure_renders_the_path_and_the_os_error() {
    let error = Error::from(jp_storage::Error::write_failed(
        Utf8Path::new("/data/conv/events.json"),
        io::Error::from(io::ErrorKind::PermissionDenied),
    ));

    assert_eq!(
        rendered(&error),
        "Failed to write file\npath=/data/conv/events.json\nerror=permission denied"
    );
}

#[cfg(unix)]
#[test]
fn a_workspace_persist_failure_keeps_the_full_cause_chain() {
    // The path a failed drop-time persist takes to the terminal: storage error
    // wrapped by the workspace, wrapped by the CLI. Each layer has to keep the
    // one below it renderable, or the user is told "IO error" and nothing else.
    let error = Error::from(crate::error::Error::Workspace(
        jp_workspace::Error::Storage(jp_storage::Error::write_failed(
            Utf8Path::new("/data/conv/events.json"),
            io::Error::from_raw_os_error(ENOSPC),
        )),
    ));

    assert_eq!(
        rendered(&error),
        "No space left on device\npath=/data/conv/events.json\nerror=No space left on device (os \
         error 28)\nsuggestion=Free up disk space and re-run. Anything after the last successful \
         write was not saved."
    );
}

#[cfg(unix)]
#[test]
fn a_bare_io_error_renders_without_a_path() {
    // `Error::Io` has no path to name, so it renders through the generic
    // cause-chain walk: an "IO error" headline plus the OS message. Pinned to
    // show what a caller gives up by not routing through
    // `jp_storage::Error::write_failed`.
    let error = Error::from(jp_storage::Error::from(io::Error::from_raw_os_error(
        ENOSPC,
    )));

    assert_eq!(
        rendered(&error),
        "IO error\n=No space left on device (os error 28)"
    );
}
