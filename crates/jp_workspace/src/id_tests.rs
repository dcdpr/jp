use std::fs;

use camino_tempfile::tempdir;

use super::*;

#[test]
fn store_is_idempotent_when_unchanged() {
    let dir = tempdir().unwrap();
    let storage = dir.path();
    let id = "abc12".parse::<Id>().unwrap();

    id.store(storage).unwrap();
    let mtime = fs::metadata(storage.join(ID_FILE))
        .unwrap()
        .modified()
        .unwrap();

    // Re-storing the same ID must not rewrite the file. A rewrite bumps the
    // mtime and wakes file watchers (e.g. `cargo watch`) on every `jp` run.
    id.store(storage).unwrap();
    let mtime_after = fs::metadata(storage.join(ID_FILE))
        .unwrap()
        .modified()
        .unwrap();

    assert_eq!(mtime, mtime_after);
}

#[test]
fn store_rewrites_when_id_changes() {
    let dir = tempdir().unwrap();
    let storage = dir.path();

    "abc12".parse::<Id>().unwrap().store(storage).unwrap();
    "xyz34".parse::<Id>().unwrap().store(storage).unwrap();

    let loaded = Id::load(storage).unwrap().unwrap();
    assert_eq!(&*loaded, "xyz34");
}
