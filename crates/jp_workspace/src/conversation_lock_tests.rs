use std::sync::{Arc, Mutex};

use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::backend::{NoopLockGuard, NullPersistBackend, PersistBackend};
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
