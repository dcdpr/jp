use std::time::Duration;

use camino_tempfile::Utf8TempDir;
use test_log::test;

use super::*;

/// The lockers that must behave identically through the trait.
fn lockers() -> Vec<(&'static str, Box<dyn ResourceLocker>, Option<Utf8TempDir>)> {
    let dir = Utf8TempDir::new().unwrap();
    vec![
        (
            "fs",
            Box::new(FsResourceLocker::new(dir.path().to_owned())),
            Some(dir),
        ),
        ("memory", Box::new(InMemoryResourceLocker::new()), None),
    ]
}

#[test]
fn test_try_lock_excludes_second_holder() {
    for (name, locker, _dir) in lockers() {
        let guard = locker.try_lock("res", None).unwrap();
        assert!(guard.is_some(), "locker: {name}");

        let second = locker.try_lock("res", None).unwrap();
        assert!(second.is_none(), "locker: {name}");

        // Independent resources are unaffected.
        let other = locker.try_lock("other", None).unwrap();
        assert!(other.is_some(), "locker: {name}");

        // Dropping the guard releases the resource.
        drop(guard);
        let third = locker.try_lock("res", None).unwrap();
        assert!(third.is_some(), "locker: {name}");
    }
}

#[test]
fn test_is_held_probe_is_non_destructive() {
    for (name, locker, _dir) in lockers() {
        assert!(!locker.is_held("res"), "locker: {name}");

        let guard = locker.try_lock("res", None).unwrap().unwrap();
        assert!(locker.is_held("res"), "locker: {name}");

        // Probing does not steal or release the lock.
        assert!(
            locker.try_lock("res", None).unwrap().is_none(),
            "locker: {name}"
        );

        drop(guard);
        assert!(!locker.is_held("res"), "locker: {name}");
    }
}

#[test]
fn test_holder_info_roundtrip() {
    for (name, locker, _dir) in lockers() {
        assert_eq!(locker.holder_info("res"), None, "locker: {name}");

        let guard = locker
            .try_lock("res", Some(r#"{"pid":42}"#))
            .unwrap()
            .unwrap();
        assert_eq!(
            locker.holder_info("res").as_deref(),
            Some(r#"{"pid":42}"#),
            "locker: {name}"
        );

        // Reacquiring without info clears what the previous holder recorded,
        // rather than attributing it to the new holder.
        drop(guard);
        let _guard = locker.try_lock("res", None).unwrap().unwrap();
        assert_eq!(locker.holder_info("res"), None, "locker: {name}");
    }
}

#[test]
fn test_blocking_lock_waits_for_release() {
    for (name, locker, _dir) in lockers() {
        let guard = locker.lock("res", None).unwrap();

        // A blocked waiter acquires the lock once (and only once) the
        // holder releases it. The holder releases after a short delay; the
        // waiter's acquisition succeeding at all proves it waited through
        // the held period.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _guard = locker.lock("res", None).unwrap();
                tx.send(()).unwrap();
            });

            // The waiter must not acquire while the guard is held.
            assert!(
                rx.recv_timeout(Duration::from_millis(100)).is_err(),
                "locker: {name}: waiter acquired a held lock"
            );

            drop(guard);
            assert!(
                rx.recv_timeout(Duration::from_secs(5)).is_ok(),
                "locker: {name}: waiter never acquired after release"
            );
        });
    }
}

#[test]
fn test_fs_remove_on_drop() {
    let dir = Utf8TempDir::new().unwrap();
    let keeping = FsResourceLocker::new(dir.path().to_owned());
    let removing = FsResourceLocker::new(dir.path().to_owned()).with_remove_on_drop();

    let guard = keeping.try_lock("kept", None).unwrap().unwrap();
    drop(guard);
    assert!(dir.path().join("kept.lock").exists());

    let guard = removing.try_lock("removed", None).unwrap().unwrap();
    assert!(dir.path().join("removed.lock").exists());
    drop(guard);
    assert!(!dir.path().join("removed.lock").exists());
}

#[test]
fn test_fs_blocking_lock_refused_when_removing_on_drop() {
    let dir = Utf8TempDir::new().unwrap();
    let locker = FsResourceLocker::new(dir.path().to_owned()).with_remove_on_drop();

    // Refused outright, and before the lock file is created: a blocking waiter
    // would park on an inode a concurrent drop is about to unlink.
    let error = locker.lock("res", None).unwrap_err();
    assert_eq!(error.resource, "res");
    assert!(!dir.path().join("res.lock").exists());
}

#[test]
fn test_fs_leftover_lock_file_does_not_block() {
    // A lock file left behind by a killed process (OS lock already
    // released) must not block the next acquisition.
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(dir.path().join("res.lock"), "stale").unwrap();

    let locker = FsResourceLocker::new(dir.path().to_owned());
    assert!(!locker.is_held("res"));
    assert!(locker.try_lock("res", None).unwrap().is_some());
}

#[test]
fn test_null_locker_never_holds() {
    let locker = NullResourceLocker;

    let _guard = locker.try_lock("res", Some("info")).unwrap().unwrap();
    // Even while a guard exists, nothing registers as held: the null
    // locker provides no exclusion at all.
    assert!(!locker.is_held("res"));
    assert_eq!(locker.holder_info("res"), None);
    assert!(locker.try_lock("res", None).unwrap().is_some());
}
