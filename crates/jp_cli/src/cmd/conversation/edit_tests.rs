use std::{fs, time::Duration};

use camino::Utf8PathBuf;
use camino_tempfile::tempdir;

use super::{Edit, ExpirationDuration};

#[test]
fn parse_now() {
    let dur: ExpirationDuration = "now".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_now_case_insensitive() {
    let dur: ExpirationDuration = "NOW".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);

    let dur: ExpirationDuration = "Now".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_humantime_duration() {
    let dur: ExpirationDuration = "1h".parse().unwrap();
    assert_eq!(dur.0, Duration::from_hours(1));

    let dur: ExpirationDuration = "30m".parse().unwrap();
    assert_eq!(dur.0, Duration::from_mins(30));

    let dur: ExpirationDuration = "0s".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_invalid() {
    let result = "not-a-duration".parse::<ExpirationDuration>();
    assert!(result.is_err());
}

/// A discarded edit must restore files to their pre-edit content, and remove
/// files that did not exist before the edit — so a malformed edit never leaves
/// the conversation in a broken state.
#[test]
fn restore_snapshots_reverts_and_removes() {
    let tmp = tempdir().unwrap();

    // A file that existed before editing: restore brings back the original.
    let existing = tmp.path().join("existing.json");
    fs::write(&existing, "original").unwrap();

    // A file that did not exist before editing.
    let created = tmp.path().join("created.json");

    // Simulate the editor having changed both (the latter into existence).
    fs::write(&existing, "garbage").unwrap();
    fs::write(&created, "garbage").unwrap();

    let snapshots: Vec<(Utf8PathBuf, Option<String>)> = vec![
        (existing.clone(), Some("original".to_owned())),
        (created.clone(), None),
    ];

    Edit::restore_snapshots(&snapshots).unwrap();

    assert_eq!(
        fs::read_to_string(&existing).unwrap(),
        "original",
        "an edited file is reverted to its pre-edit content"
    );
    assert!(
        !created.exists(),
        "a file created by the edit is removed on revert"
    );
}
