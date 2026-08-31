use std::time::Duration;

use super::{Mark, append, latest_run, load, overlapping, sweep};
use crate::debug_app::capture::unix_millis;

fn mark(run: &str, step: usize, began_ms: u64, ended_ms: u64) -> Mark {
    Mark {
        run: run.to_owned(),
        step,
        label: format!("select sidebar.row.{step}"),
        began_ms,
        ended_ms,
    }
}

#[test]
fn a_mark_holds_the_moments_inside_its_window() {
    let mark = mark("drive-1", 1, 1_000, 2_000);

    assert!(mark.holds(1_000));
    assert!(mark.holds(1_500));
    assert!(mark.holds(1_999));
    assert!(!mark.holds(999));
    assert!(!mark.holds(2_001));
}

/// Two steps meeting in one millisecond is the ordinary case with `reads:
/// "none"`, where only bookkeeping separates one step's last reading of the
/// clock from the next one's first.
///
/// An interval beginning at that instant belongs to the step that was starting,
/// not to both.
/// Counted twice it inflates exactly the numbers a caller compares between
/// runs.
#[test]
fn a_moment_two_steps_share_belongs_to_the_later_one() {
    let first = mark("drive-1", 1, 1_000, 2_000);
    let second = mark("drive-1", 2, 2_000, 3_000);

    assert!(!first.holds(2_000));
    assert!(second.holds(2_000));
}

#[test]
fn marks_round_trip_and_accumulate_across_runs() {
    let dir = camino_tempfile::tempdir().unwrap();

    append(dir.path(), &[mark("drive-1", 1, 1_000, 2_000)]).unwrap();
    append(dir.path(), &[
        mark("drive-2", 1, 5_000, 6_000),
        mark("drive-2", 2, 6_000, 7_000),
    ])
    .unwrap();

    let loaded = load(dir.path());

    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0], mark("drive-1", 1, 1_000, 2_000));
    assert_eq!(loaded[2], mark("drive-2", 2, 6_000, 7_000));
}

/// A step number repeats between runs, so "step 1" alone is ambiguous and the
/// most recent run is what a caller asking about the drive they just did means.
#[test]
fn the_latest_run_is_the_last_one_appended() {
    let marks = vec![
        mark("drive-1", 1, 1_000, 2_000),
        mark("drive-2", 1, 5_000, 6_000),
        mark("drive-2", 2, 6_000, 7_000),
    ];

    let latest = latest_run(&marks);

    assert_eq!(latest.len(), 2);
    assert!(latest.iter().all(|mark| mark.run == "drive-2"));
}

#[test]
fn nothing_driven_has_no_latest_run() {
    assert_eq!(latest_run(&[]), Vec::new());
}

#[test]
fn overlapping_keeps_a_step_that_straddles_the_window_edge() {
    let marks = vec![
        mark("drive-1", 1, 1_000, 2_000),
        mark("drive-1", 2, 2_000, 3_000),
        mark("drive-1", 3, 9_000, 9_500),
    ];

    let found = overlapping(&marks, 2_500, 4_000);

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].step, 2);
}

/// One file holds every run a slot has driven, so expiry is by line: deleting
/// the file would take the runs still inside the window with it.
#[test]
fn sweeping_drops_the_expired_runs_and_keeps_the_rest() {
    let dir = camino_tempfile::tempdir().unwrap();
    let now = unix_millis();
    let hour = 60 * 60 * 1000;

    append(dir.path(), &[
        mark("drive-old", 1, now - 5 * hour, now - 5 * hour + 100),
        mark("drive-recent", 1, now - hour, now - hour + 100),
        mark("drive-recent", 2, now - hour + 100, now - hour + 200),
    ])
    .unwrap();

    assert_eq!(sweep(dir.path(), Duration::from_hours(2)), 1);

    let kept = load(dir.path());
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|mark| mark.run == "drive-recent"));
}

/// A slot driven today has nothing to expire, and its file is left untouched
/// rather than rewritten.
#[test]
fn sweeping_a_slot_with_nothing_expired_rewrites_nothing() {
    let dir = camino_tempfile::tempdir().unwrap();
    let now = unix_millis();
    append(dir.path(), &[mark("drive-1", 1, now - 1_000, now - 900)]).unwrap();

    let before = std::fs::read_to_string(super::path(dir.path())).unwrap();

    assert_eq!(sweep(dir.path(), Duration::from_hours(1)), 0);
    assert_eq!(
        std::fs::read_to_string(super::path(dir.path())).unwrap(),
        before
    );
}

#[test]
fn sweeping_a_slot_that_never_drove_anything_finds_nothing() {
    let dir = camino_tempfile::tempdir().unwrap();

    assert_eq!(sweep(dir.path(), Duration::from_mins(1)), 0);
}

#[test]
fn a_malformed_line_is_skipped_rather_than_fatal() {
    let dir = camino_tempfile::tempdir().unwrap();
    append(dir.path(), &[mark("drive-1", 1, 1_000, 2_000)]).unwrap();

    let path = super::path(dir.path());
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{raw}{{\"run\": truncated")).unwrap();

    assert_eq!(load(dir.path()).len(), 1);
}
