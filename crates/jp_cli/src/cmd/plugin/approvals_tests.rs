use camino_tempfile::{Utf8TempDir, tempdir};
use chrono::{DateTime, Utc};
use jp_plugin::Manifest;
use pretty_assertions::assert_eq;

use super::*;
use crate::cmd::plugin::discovery::Location;

fn at() -> DateTime<Utc> {
    "2026-09-28T10:12:00Z".parse().unwrap()
}

fn approval(path: &Utf8Path, sha256: &str) -> ApprovedPlugin {
    ApprovedPlugin {
        path: path.to_owned(),
        sha256: sha256.to_owned(),
        approved_at: at(),
        installed: false,
        manifest: None,
    }
}

fn store(tmp: &Utf8TempDir) -> ApprovalStore {
    ApprovalStore::load_from(Some(tmp.path().join("approvals.json")))
}

#[test]
fn an_approval_answers_for_that_file_with_those_contents() {
    let tmp = tempdir().unwrap();
    let binary = tmp.path().join("jp-foo");
    fs::write(&binary, "a").unwrap();

    let mut store = store(&tmp);
    store.record("foo", approval(&binary, "aaa")).unwrap();

    assert_eq!(store.check("foo", &binary, "aaa"), ApprovalMatch::Matches);
    assert_eq!(store.check("foo", &binary, "bbb"), ApprovalMatch::Changed);
    assert_eq!(store.check("bar", &binary, "aaa"), ApprovalMatch::None);
}

/// The shadowing case: a second binary with the approved name elsewhere on
/// `$PATH` is told apart from the approved one.
#[test]
fn another_file_with_the_approved_name_is_not_approved() {
    let tmp = tempdir().unwrap();
    let approved = tmp.path().join("jp-foo");
    let other = tmp.path().join("other-jp-foo");
    fs::write(&approved, "a").unwrap();
    fs::write(&other, "a").unwrap();

    let mut store = store(&tmp);
    store.record("foo", approval(&approved, "aaa")).unwrap();

    assert_eq!(
        store.check("foo", &other, "aaa"),
        ApprovalMatch::Elsewhere(approved)
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_to_the_approved_file_is_that_file() {
    let tmp = tempdir().unwrap();
    let approved = tmp.path().join("jp-foo");
    let link = tmp.path().join("link");
    fs::write(&approved, "a").unwrap();
    std::os::unix::fs::symlink(&approved, &link).unwrap();

    let mut store = store(&tmp);
    store.record("foo", approval(&approved, "aaa")).unwrap();

    assert_eq!(store.check("foo", &link, "aaa"), ApprovalMatch::Matches);
}

/// Two `jp` runs approving different plugins must not undo each other.
#[test]
fn recording_keeps_what_another_process_recorded_since() {
    let tmp = tempdir().unwrap();
    let mut first = store(&tmp);
    let mut second = store(&tmp);

    first.record("a", approval("/a".into(), "1")).unwrap();
    second.record("b", approval("/b".into(), "2")).unwrap();

    let reread = store(&tmp);
    assert!(reread.get("a").is_some());
    assert!(reread.get("b").is_some());
}

#[test]
fn removing_an_approval_returns_it() {
    let tmp = tempdir().unwrap();
    let mut store = store(&tmp);
    store.record("a", approval("/a".into(), "1")).unwrap();

    assert_eq!(store.remove("a").unwrap(), Some(approval("/a".into(), "1")));
    assert_eq!(store.remove("a").unwrap(), None);
    assert!(store.get("a").is_none());
}

#[test]
fn a_malformed_store_is_treated_as_empty() {
    let tmp = tempdir().unwrap();
    fs::write(tmp.path().join("approvals.json"), "{ not json").unwrap();

    assert!(store(&tmp).get("a").is_none());

    // Recording over it starts a fresh store.
    let mut fresh = store(&tmp);
    fresh.record("a", approval("/a".into(), "1")).unwrap();
    assert!(store(&tmp).get("a").is_some());
}

fn packed(path: Utf8PathBuf) -> LocalPlugin {
    LocalPlugin {
        name: "packed".to_owned(),
        path,
        location: Location::Path,
        manifest: ManifestState::Missing,
    }
}

fn recorded_manifest() -> Manifest {
    Manifest {
        protocol: 1,
        description: "Packed".to_owned(),
        command: vec!["packed".to_owned()],
    }
}

#[test]
fn a_binary_without_a_manifest_takes_the_one_its_approval_recorded() {
    let tmp = tempdir().unwrap();
    let binary = tmp.path().join("jp-packed");
    fs::write(&binary, "packed bytes").unwrap();
    let sha256 = registry::sha256_file(&binary).unwrap();

    let mut store = store(&tmp);
    store
        .record("packed", ApprovedPlugin {
            manifest: Some(recorded_manifest()),
            ..approval(&binary, &sha256)
        })
        .unwrap();

    let mut local = [packed(binary)];
    store.apply_recorded(&mut local);

    assert_eq!(
        local[0].manifest,
        ManifestState::Recorded(recorded_manifest())
    );
}

/// What a binary claims comes from the approved bytes; a changed binary has to
/// be approved again before it claims anything.
#[test]
fn a_changed_binary_does_not_take_the_recorded_manifest() {
    let tmp = tempdir().unwrap();
    let binary = tmp.path().join("jp-packed");
    fs::write(&binary, "packed bytes").unwrap();

    let mut store = store(&tmp);
    store
        .record("packed", ApprovedPlugin {
            manifest: Some(recorded_manifest()),
            ..approval(&binary, "the old contents")
        })
        .unwrap();

    let mut local = [packed(binary)];
    store.apply_recorded(&mut local);

    assert_eq!(local[0].manifest, ManifestState::Missing);
}
