use camino_tempfile::{Utf8TempDir, tempdir};
use datetime_literal::datetime;
use jp_conversation::ConversationId;
use test_log::test;

use super::*;
use crate::{
    Workspace,
    session::{Session, SessionId, SessionSource},
};

fn test_session() -> Session {
    Session {
        id: SessionId::new("12345").unwrap(),
        source: SessionSource::Getsid,
    }
}

/// Create a workspace with both workspace and user storage configured.
fn setup() -> (Utf8TempDir, Workspace) {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");
    let user_root = tmp.path().join("user");

    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap()
        .with_local_storage_at(&user_root, "test-ws", "abc")
        .unwrap();
    ws.disable_persistence();

    (tmp, ws)
}

#[test]
fn activate_creates_new_mapping() {
    let (_tmp, ws) = setup();
    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let now = datetime!(2025-07-19 14:30:00 Z);

    ws.activate_session_conversation(&session, id, now).unwrap();

    assert_eq!(ws.session_active_conversation(&session), Some(id));
    assert_eq!(ws.session_previous_conversation(&session), None);
}

#[test]
fn activate_deduplicates_history() {
    let (_tmp, ws) = setup();
    let session = test_session();
    let id1 = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2025-07-19 15:00:00 Z)).unwrap();

    ws.activate_session_conversation(&session, id1, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    ws.activate_session_conversation(&session, id2, datetime!(2025-07-19 15:00:00 Z))
        .unwrap();
    // Re-activate id1 — should move it to the front, not duplicate.
    ws.activate_session_conversation(&session, id1, datetime!(2025-07-19 16:00:00 Z))
        .unwrap();

    assert_eq!(ws.session_active_conversation(&session), Some(id1));
    assert_eq!(ws.session_previous_conversation(&session), Some(id2));

    // Verify dedup by reloading from disk.
    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.history.len(), 2);
    assert_eq!(
        mapping.history[0].activated_at,
        datetime!(2025-07-19 16:00:00 Z)
    );
}

#[test]
fn load_returns_none_when_missing() {
    let (_tmp, ws) = setup();
    let session = test_session();

    assert!(ws.session_active_conversation(&session).is_none());
}

#[test]
fn previous_conversation_id_with_single_entry() {
    let mut mapping = SessionMapping::new(SessionSource::Getsid);
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();
    mapping.activate(id, datetime!(2025-07-19 14:00:00 Z));

    assert_eq!(mapping.active_conversation_id(), Some(id));
    assert_eq!(mapping.previous_conversation_id(), None);
}

#[test]
fn env_source_roundtrips_through_json() {
    let (_tmp, ws) = setup();
    let session = Session {
        id: SessionId::new("my-session").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();

    let mapping = ws.load_session_mapping(&session).unwrap();
    assert_eq!(mapping.source, SessionSource::Env {
        key: "JP_SESSION".to_owned()
    });
    assert_eq!(mapping.active_conversation_id(), Some(id));
}

#[test]
fn no_user_storage_returns_none() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");

    // Workspace without user storage.
    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap();
    ws.disable_persistence();

    let session = test_session();
    assert!(ws.session_active_conversation(&session).is_none());
}

#[test]
fn no_user_storage_returns_error_on_write() {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join("storage");

    let mut ws = Workspace::new(tmp.path())
        .persisted_at(&storage_path)
        .unwrap();
    ws.disable_persistence();

    let session = test_session();
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    assert!(
        ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
            .is_err()
    );
}

#[test]
fn env_source_is_never_considered_dead() {
    let source = SessionSource::env("JP_SESSION");
    assert!(!is_session_process_dead(&source, "anything"));
}

#[cfg(unix)]
#[test]
fn getsid_with_own_pid_is_alive() {
    let pid = std::process::id().to_string();
    assert!(!is_pid_dead(&pid));
}

#[cfg(unix)]
#[test]
fn getsid_with_nonexistent_pid_is_dead() {
    // PID 2_000_000_000 is extremely unlikely to exist.
    assert!(is_pid_dead("2000000000"));
}

#[cfg(unix)]
#[test]
fn getsid_with_unparseable_key_is_not_dead() {
    assert!(!is_pid_dead("not-a-pid"));
}

#[cfg(windows)]
#[test]
fn hwnd_with_own_console_is_alive() {
    // GetConsoleWindow returns the HWND for the current console.
    // It should be reported as alive.
    let hwnd = unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() };
    if !hwnd.is_null() {
        let key = format!("{}", hwnd as isize);
        assert!(!is_hwnd_dead(&key));
    }
    // If hwnd is null (no console, e.g. GUI-only CI), skip silently.
}

#[cfg(windows)]
#[test]
fn hwnd_with_nonexistent_handle_is_dead() {
    // 0xDEAD is extremely unlikely to be a valid window handle.
    assert!(is_hwnd_dead("57005"));
}

#[cfg(windows)]
#[test]
fn hwnd_with_unparseable_key_is_not_dead() {
    assert!(!is_hwnd_dead("not-a-handle"));
}

#[cfg(unix)]
#[test]
fn cleanup_removes_stale_getsid_session() {
    let (_tmp, mut ws) = setup();
    let session = Session {
        id: SessionId::new("2000000000").unwrap(), // nonexistent PID
        source: SessionSource::Getsid,
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    // Create a conversation so the workspace has content.
    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    // Write a session mapping referencing a live conversation.
    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    assert!(ws.session_active_conversation(&session).is_some());

    // Cleanup should remove it because PID 2000000000 is dead,
    // even though the conversation still exists.
    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session).is_none(),
        "Stale getsid session should be cleaned up when process is dead"
    );
}

#[cfg(windows)]
#[test]
fn cleanup_removes_stale_hwnd_session() {
    let (_tmp, mut ws) = setup();
    let session = Session {
        id: SessionId::new("57005").unwrap(), // 0xDEAD — unlikely to be a valid HWND
        source: SessionSource::Hwnd,
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();
    assert!(ws.session_active_conversation(&session).is_some());

    // Cleanup should remove it because HWND 0xDEAD is not a valid window.
    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session).is_none(),
        "Stale hwnd session should be cleaned up when window handle is dead"
    );
}

#[test]
fn cleanup_keeps_env_session_with_live_conversations() {
    let (_tmp, mut ws) = setup();
    let session = Session {
        id: SessionId::new("my-ci-session").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let id = ConversationId::try_from(datetime!(2025-07-19 14:00:00 Z)).unwrap();

    let config = std::sync::Arc::new(jp_config::AppConfig::new_test());
    ws.create_conversation_with_id(id, jp_conversation::Conversation::default(), config);

    ws.activate_session_conversation(&session, id, datetime!(2025-07-19 14:00:00 Z))
        .unwrap();

    ws.cleanup_stale_files();

    assert!(
        ws.session_active_conversation(&session).is_some(),
        "Env session with live conversations should not be cleaned up"
    );
}
