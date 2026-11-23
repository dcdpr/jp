use std::fs::{self, TryLockError};

use camino_tempfile::Utf8TempDir;

use super::*;

#[test]
fn a_missing_directory_takes_no_lock() {
    let dir = Utf8TempDir::new().unwrap();

    let lock = exclusive(&dir.path().join("missing/.lock")).unwrap();

    assert!(lock.is_none());
    assert!(!dir.path().join("missing").exists());
}

/// The file outlives the lock: a writer waiting on it must find the inode the
/// next writer locks.
#[test]
fn the_lock_file_is_created_and_kept() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(".lock");

    let lock = exclusive(&path).unwrap();
    assert!(lock.is_some());
    drop(lock);

    assert_eq!(fs::read_to_string(&path).unwrap(), "");
}

#[test]
fn trying_takes_a_free_lock_and_leaves_a_held_one() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(".lock");

    let held = try_exclusive(&path).unwrap().unwrap();
    assert!(try_exclusive(&path).unwrap().is_none());

    drop(held);
    assert!(try_exclusive(&path).unwrap().is_some());
}

#[test]
fn trying_in_a_missing_directory_is_an_error() {
    let dir = Utf8TempDir::new().unwrap();

    let error = try_exclusive(&dir.path().join("missing/.lock")).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn the_lock_is_held_until_dropped() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join(".lock");
    let held = exclusive(&path).unwrap().unwrap();

    let other = File::open(&path).unwrap();
    assert!(matches!(other.try_lock(), Err(TryLockError::WouldBlock)));

    drop(held);
    other.try_lock().unwrap();
}
