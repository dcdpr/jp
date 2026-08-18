use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::backend::InMemoryStorageBackend;

#[test]
fn read_only_session_backend_serves_reads_from_the_inner_backend() {
    let inner = Arc::new(InMemoryStorageBackend::new());
    inner
        .save_session("getsid-12345", &json!({ "history": [] }))
        .unwrap();

    let backend = ReadOnlySessionBackend::new(inner);

    assert_eq!(
        backend.load_session("getsid-12345").unwrap(),
        Some(json!({ "history": [] }))
    );
    assert_eq!(backend.list_session_keys(), vec!["getsid-12345".to_owned()]);
}

#[test]
fn read_only_session_backend_discards_writes() {
    let inner = Arc::new(InMemoryStorageBackend::new());
    let backend = ReadOnlySessionBackend::new(inner.clone());

    backend
        .save_session("getsid-12345", &json!({ "history": [] }))
        .unwrap();

    assert_eq!(inner.load_session("getsid-12345").unwrap(), None);
    assert!(backend.list_session_keys().is_empty());
}

#[test]
fn read_only_session_backend_write_does_not_clobber_an_existing_mapping() {
    let inner = Arc::new(InMemoryStorageBackend::new());
    inner
        .save_session("getsid-12345", &json!({ "history": ["original"] }))
        .unwrap();

    let backend = ReadOnlySessionBackend::new(inner.clone());
    backend
        .save_session("getsid-12345", &json!({ "history": ["replacement"] }))
        .unwrap();

    assert_eq!(
        inner.load_session("getsid-12345").unwrap(),
        Some(json!({ "history": ["original"] }))
    );
}
