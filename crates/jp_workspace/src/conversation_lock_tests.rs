use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
    },
};

use camino::Utf8Path;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::backend::{NoopLockGuard, NullPersistBackend, PersistBackend, Projection};
use parking_lot::RwLock;

use super::*;
use crate::handle::ConversationHandle;

/// Mock persistence backend that records all write/remove calls.
#[derive(Debug, Default)]
struct MockPersistBackend {
    writes: Mutex<Vec<(ConversationId, Conversation, ConversationStream)>>,
    removes: Mutex<Vec<ConversationId>>,
}

impl MockPersistBackend {
    fn new() -> Self {
        Self::default()
    }

    fn writes(&self) -> Vec<(ConversationId, Conversation, ConversationStream)> {
        self.writes.lock().unwrap().clone()
    }
}

impl PersistBackend for MockPersistBackend {
    fn write(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
        _projection: Projection,
    ) -> Result<(), jp_storage::Error> {
        self.writes
            .lock()
            .unwrap()
            .push((*id, metadata.clone(), events.clone()));
        Ok(())
    }

    fn remove(&self, id: &ConversationId) -> Result<(), jp_storage::Error> {
        self.removes.lock().unwrap().push(*id);
        Ok(())
    }

    fn archive(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }

    fn unarchive(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }
}

/// Persistence backend that fails every write with a fixed error, counting the
/// attempts.
///
/// The count is what makes the "one failing disk, one report" claim testable: a
/// backend that only records the error cannot distinguish "the write was
/// skipped" from "the write was retried and failed again".
#[derive(Debug)]
struct FailingPersistBackend {
    attempts: AtomicUsize,
    out_of_space: bool,
}

impl FailingPersistBackend {
    fn out_of_space() -> Self {
        Self {
            attempts: AtomicUsize::new(0),
            out_of_space: true,
        }
    }

    fn permission_denied() -> Self {
        Self {
            attempts: AtomicUsize::new(0),
            out_of_space: false,
        }
    }

    fn attempts(&self) -> usize {
        self.attempts.load(AtomicOrdering::Relaxed)
    }
}

impl PersistBackend for FailingPersistBackend {
    fn write(
        &self,
        _id: &ConversationId,
        _metadata: &Conversation,
        _events: &ConversationStream,
        _projection: Projection,
    ) -> Result<(), jp_storage::Error> {
        self.attempts.fetch_add(1, AtomicOrdering::Relaxed);

        let source = if self.out_of_space {
            io::Error::from(io::ErrorKind::StorageFull)
        } else {
            io::Error::from(io::ErrorKind::PermissionDenied)
        };
        Err(jp_storage::Error::write_failed(
            Utf8Path::new("/data/conv/events.json"),
            source,
        ))
    }

    fn remove(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }

    fn archive(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }

    fn unarchive(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }
}

fn test_lock_with_failing_backend(
    backend: FailingPersistBackend,
) -> (ConversationLock, Arc<FailingPersistBackend>) {
    let backend = Arc::new(backend);
    let lock = ConversationLock::new(
        test_handle(),
        Arc::new(RwLock::new(Conversation::default())),
        Arc::new(RwLock::new(ConversationStream::new_test())),
        Arc::clone(&backend) as _,
        Box::new(NoopLockGuard),
        Projection::Projected,
    );
    (lock, backend)
}

fn test_id() -> ConversationId {
    ConversationId::try_from(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH).unwrap()
}

fn test_handle() -> ConversationHandle {
    ConversationHandle::new(test_id())
}

fn test_lock_with_mock() -> (ConversationLock, Arc<MockPersistBackend>) {
    let mock = Arc::new(MockPersistBackend::new());
    let lock = ConversationLock::new(
        test_handle(),
        Arc::new(RwLock::new(Conversation::default())),
        Arc::new(RwLock::new(ConversationStream::new_test())),
        Arc::clone(&mock) as _,
        Box::new(NoopLockGuard),
        Projection::Projected,
    );
    (lock, mock)
}

fn test_lock_no_writer() -> ConversationLock {
    ConversationLock::new(
        test_handle(),
        Arc::new(RwLock::new(Conversation::default())),
        Arc::new(RwLock::new(ConversationStream::new_test())),
        Arc::new(NullPersistBackend),
        Box::new(NoopLockGuard),
        Projection::Projected,
    )
}

#[test]
fn lock_id_matches() {
    let lock = test_lock_no_writer();
    assert_eq!(lock.id(), test_id());
}

#[test]
fn lock_metadata_readable() {
    let lock = test_lock_no_writer();
    assert_eq!(lock.metadata().title, None);
}

#[test]
fn lock_events_readable() {
    let lock = test_lock_no_writer();
    assert!(lock.events().is_empty());
}

#[test]
fn as_mut_does_not_consume_lock() {
    let lock = test_lock_no_writer();
    let _conv = lock.as_mut();
    // lock is still usable after as_mut
    assert_eq!(lock.id(), test_id());
}

#[test]
fn into_mut_consumes_lock() {
    let lock = test_lock_no_writer();
    let conv = lock.into_mut();
    // lock is consumed, conv owns the flock
    assert_eq!(conv.id(), test_id());
}

#[test]
fn fresh_conv_is_not_dirty() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    assert!(!conv.is_dirty());
}

#[test]
fn update_metadata_sets_dirty() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    conv.update_metadata(|_| {});
    assert!(conv.is_dirty());
}

#[test]
fn update_events_sets_dirty() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    conv.update_events(|_| {});
    assert!(conv.is_dirty());
}

#[test]
fn update_sets_dirty() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    conv.update(|_, _| {});
    assert!(conv.is_dirty());
}

#[test]
fn clear_dirty_resets_flag() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    conv.update_metadata(|_| {});
    assert!(conv.is_dirty());
    conv.clear_dirty();
    assert!(!conv.is_dirty());
}

#[test]
fn update_metadata_forwards_return_value() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    let title = conv.update_metadata(|m| {
        m.title = Some("hello".to_string());
        m.title.clone()
    });
    assert_eq!(title, Some("hello".to_string()));
}

#[test]
fn update_events_forwards_result() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    let result: Result<(), &str> = conv.update_events(|_| Err("fail"));
    assert!(result.is_err());
}

#[test]
fn flush_skips_when_not_dirty() {
    let (lock, mock) = test_lock_with_mock();
    let mut conv = lock.into_mut();
    conv.flush().unwrap();
    assert_eq!(mock.writes().len(), 0);
}

#[test]
fn flush_writes_when_dirty() {
    let (lock, mock) = test_lock_with_mock();
    let mut conv = lock.into_mut();
    conv.update_metadata(|m| m.title = Some("flushed".into()));
    conv.flush().unwrap();
    assert_eq!(mock.writes().len(), 1);
    assert_eq!(mock.writes()[0].1.title.as_deref(), Some("flushed"));
}

#[test]
fn flush_clears_dirty_flag() {
    let (lock, _mock) = test_lock_with_mock();
    let mut conv = lock.into_mut();
    conv.update_metadata(|_| {});
    assert!(conv.is_dirty());
    conv.flush().unwrap();
    assert!(!conv.is_dirty());
}

#[test]
fn double_flush_writes_once() {
    let (lock, mock) = test_lock_with_mock();
    let mut conv = lock.into_mut();
    conv.update_metadata(|_| {});
    conv.flush().unwrap();
    conv.flush().unwrap(); // not dirty anymore
    assert_eq!(mock.writes().len(), 1);
}

#[test]
fn drop_persists_dirty_conv() {
    let (lock, mock) = test_lock_with_mock();
    let conv = lock.into_mut();
    conv.update_metadata(|m| m.title = Some("dropped".into()));
    drop(conv);
    assert_eq!(mock.writes().len(), 1);
    assert_eq!(mock.writes()[0].1.title.as_deref(), Some("dropped"));
}

#[test]
fn drop_skips_clean_conv() {
    let (lock, mock) = test_lock_with_mock();
    let conv = lock.into_mut();
    drop(conv);
    assert_eq!(mock.writes().len(), 0);
}

#[test]
fn drop_skips_after_flush() {
    let (lock, mock) = test_lock_with_mock();
    let mut conv = lock.into_mut();
    conv.update_metadata(|_| {});
    conv.flush().unwrap();
    drop(conv);
    // Only the flush write, not a second drop write.
    assert_eq!(mock.writes().len(), 1);
}

#[test]
fn drop_skips_after_clear_dirty() {
    let (lock, mock) = test_lock_with_mock();
    let conv = lock.into_mut();
    conv.update_metadata(|_| {});
    conv.clear_dirty();
    drop(conv);
    assert_eq!(mock.writes().len(), 0);
}

#[test]
fn drop_skips_without_writer() {
    let lock = test_lock_no_writer();
    let conv = lock.into_mut();
    conv.update_metadata(|_| {});
    drop(conv); // no writer, should not panic
}

#[test]
fn metadata_read_reflects_mutations() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    conv.update_metadata(|m| m.title = Some("updated".into()));
    assert_eq!(conv.metadata().title.as_deref(), Some("updated"));
}

#[test]
fn events_read_reflects_mutations() {
    let lock = test_lock_no_writer();
    let conv = lock.as_mut();
    conv.update_events(ConversationStream::sanitize);
    // Just verify we can read after mutation without deadlock.
    let _events = conv.events();
}

#[test]
fn as_mut_mutations_visible_through_lock() {
    let lock = test_lock_no_writer();
    {
        let conv = lock.as_mut();
        conv.update_metadata(|m| m.title = Some("visible".into()));
    }
    assert_eq!(lock.metadata().title.as_deref(), Some("visible"));
}

#[test]
fn no_persist_failure_recorded_on_a_healthy_backend() {
    let (lock, _mock) = test_lock_with_mock();
    {
        let conv = lock.as_mut();
        conv.update_metadata(|m| m.title = Some("fine".into()));
    }
    assert!(lock.take_persist_failure().is_none());
}

#[test]
fn drop_records_persist_failure_on_the_lock() {
    let (lock, backend) = test_lock_with_failing_backend(FailingPersistBackend::out_of_space());

    {
        let conv = lock.as_mut();
        conv.update_metadata(|m| m.title = Some("unsaved".into()));
    }

    assert_eq!(backend.attempts(), 1);
    let error = lock
        .take_persist_failure()
        .expect("the failed drop-time write is recorded on the lock");
    assert!(error.is_out_of_space());
    assert_eq!(
        error.to_string(),
        "Storage error: no space left on device while writing /data/conv/events.json"
    );
}

#[test]
fn recorded_persist_failure_is_returned_only_once() {
    let (lock, _backend) = test_lock_with_failing_backend(FailingPersistBackend::out_of_space());

    {
        let conv = lock.as_mut();
        conv.update_metadata(|_| {});
    }

    assert!(lock.take_persist_failure().is_some());
    assert!(
        lock.take_persist_failure().is_none(),
        "draining the failure must not report it a second time"
    );
}

#[test]
fn out_of_space_skips_every_later_write_attempt() {
    let (lock, backend) = test_lock_with_failing_backend(FailingPersistBackend::out_of_space());

    // Three mutation scopes, each of which would persist on drop.
    for _ in 0..3 {
        let conv = lock.as_mut();
        conv.update_metadata(|_| {});
    }

    assert_eq!(
        backend.attempts(),
        1,
        "a full disk must be hit once, not once per mutation scope"
    );
}

#[test]
fn out_of_space_records_only_the_first_failure() {
    let (lock, _backend) = test_lock_with_failing_backend(FailingPersistBackend::out_of_space());

    {
        let conv = lock.as_mut();
        conv.update_metadata(|m| m.title = Some("first".into()));
    }
    {
        let conv = lock.as_mut();
        conv.update_metadata(|m| m.title = Some("second".into()));
    }

    assert!(lock.take_persist_failure().is_some());
    assert!(lock.take_persist_failure().is_none());
}

#[test]
fn a_recoverable_failure_does_not_skip_later_writes() {
    // Only a full filesystem makes further writes futile. A permission error may
    // clear (the user chmods the directory mid-run), so each scope still tries.
    let (lock, backend) =
        test_lock_with_failing_backend(FailingPersistBackend::permission_denied());

    for _ in 0..3 {
        let conv = lock.as_mut();
        conv.update_metadata(|_| {});
    }

    assert_eq!(backend.attempts(), 3);
    let error = lock.take_persist_failure().expect("failure is recorded");
    assert!(!error.is_out_of_space());
}

#[test]
fn flush_returns_a_failure_recorded_by_an_earlier_drop() {
    let (lock, backend) = test_lock_with_failing_backend(FailingPersistBackend::out_of_space());

    {
        let conv = lock.as_mut();
        conv.update_metadata(|_| {});
    }
    assert_eq!(backend.attempts(), 1);

    let mut conv = lock.as_mut();
    let error = conv
        .flush()
        .expect_err("flush surfaces the earlier drop-time failure");

    assert!(error.is_out_of_space());
    assert_eq!(
        backend.attempts(),
        1,
        "flush returns the recorded failure without touching a full disk again"
    );
    assert!(
        lock.take_persist_failure().is_none(),
        "a failure surfaced through flush is not reported a second time"
    );
}

#[test]
fn flush_does_not_record_its_own_failure() {
    // `flush` propagates, so its caller owns reporting. Recording it too would
    // produce the double report this mechanism exists to prevent.
    let (lock, _backend) = test_lock_with_failing_backend(FailingPersistBackend::out_of_space());

    let mut conv = lock.as_mut();
    conv.update_metadata(|_| {});
    assert!(conv.flush().is_err());
    drop(conv);

    assert!(lock.take_persist_failure().is_none());
}

#[test]
fn a_failed_flush_leaves_the_scope_dirty_for_the_drop_safety_net() {
    let (lock, backend) =
        test_lock_with_failing_backend(FailingPersistBackend::permission_denied());

    let mut conv = lock.as_mut();
    conv.update_metadata(|_| {});
    assert!(conv.flush().is_err());
    assert!(conv.is_dirty());
    drop(conv);

    assert_eq!(
        backend.attempts(),
        2,
        "the drop safety net retries a recoverable flush failure"
    );
}

#[test]
fn a_scope_drains_a_failure_recorded_before_it_existed() {
    // The lock-owning scope (`into_mut`) is the only drain point once the lock
    // has been consumed, so it must see failures recorded by earlier scopes.
    let (lock, _backend) = test_lock_with_failing_backend(FailingPersistBackend::out_of_space());

    {
        let conv = lock.as_mut();
        conv.update_metadata(|_| {});
    }

    let conv = lock.into_mut();
    let error = conv
        .take_persist_failure()
        .expect("the owning scope sees the earlier failure");
    assert!(error.is_out_of_space());
    assert!(conv.take_persist_failure().is_none());
}

#[test]
fn multiple_as_mut_each_persist_independently() {
    let (lock, mock) = test_lock_with_mock();

    {
        let conv = lock.as_mut();
        conv.update_metadata(|m| m.title = Some("first".into()));
    } // persist #1

    {
        let conv = lock.as_mut();
        conv.update_metadata(|m| m.title = Some("second".into()));
    } // persist #2

    assert_eq!(mock.writes().len(), 2);
    assert_eq!(mock.writes()[0].1.title.as_deref(), Some("first"));
    assert_eq!(mock.writes()[1].1.title.as_deref(), Some("second"));
}
