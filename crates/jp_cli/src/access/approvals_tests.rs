use chrono::TimeZone as _;

use super::*;

fn ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 26, 13, 0, 0).unwrap()
}

#[test]
fn lookup_distinguishes_approved_retargeted_unknown() {
    let mut store = ApprovalStore::default();
    store.record("fork", Utf8PathBuf::from("/code/forks/serde"), ts());

    assert_eq!(
        store.lookup("fork", Utf8Path::new("/code/forks/serde")),
        ApprovalLookup::Approved
    );
    assert_eq!(
        store.lookup("fork", Utf8Path::new("/etc/passwd")),
        ApprovalLookup::Retargeted {
            previous: Utf8PathBuf::from("/code/forks/serde"),
        }
    );
    assert_eq!(
        store.lookup("other", Utf8Path::new("/code/forks/serde")),
        ApprovalLookup::Unknown
    );
}

#[test]
fn record_replaces_existing_target() {
    let mut store = ApprovalStore::default();
    store.record("fork", Utf8PathBuf::from("/a"), ts());
    store.record("fork", Utf8PathBuf::from("/b"), ts());

    // The new target is approved; the old one now reads as a retarget.
    assert_eq!(
        store.lookup("fork", Utf8Path::new("/b")),
        ApprovalLookup::Approved
    );
    assert_eq!(
        store.lookup("fork", Utf8Path::new("/a")),
        ApprovalLookup::Retargeted {
            previous: Utf8PathBuf::from("/b"),
        }
    );
}

#[test]
fn save_then_load_round_trips() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("approvals.json");

    let mut store = ApprovalStore::default();
    store.record("fork", Utf8PathBuf::from("/code/forks/serde"), ts());
    store.save(&path).unwrap();

    let loaded = ApprovalStore::load(&path);
    assert_eq!(loaded, store);
}

#[test]
fn load_missing_file_is_empty() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("nope.json");
    assert_eq!(ApprovalStore::load(&path), ApprovalStore::default());
}

#[test]
fn load_malformed_file_is_empty() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("approvals.json");
    std::fs::write(&path, "{ not json").unwrap();
    assert_eq!(ApprovalStore::load(&path), ApprovalStore::default());
}
