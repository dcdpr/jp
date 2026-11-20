use std::{fs, time::Duration};

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use chrono::{DateTime, Utc};

use super::*;

/// A backend with user-local storage and a conversation directory in it,
/// holding the draft `jp query` would have composed.
///
/// The temp dir comes back so the caller keeps it alive.
fn draft_fixture() -> (FsStorageBackend, ConversationId, Utf8PathBuf, Utf8TempDir) {
    let tmp = camino_tempfile::tempdir().unwrap();
    let fs = FsStorageBackend::new(&tmp.path().join(".jp"))
        .unwrap()
        .with_user_storage(&tmp.path().join("user"), None, "abc")
        .unwrap();

    let id =
        ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .unwrap();
    let dir = fs.build_conversation_dir(&id, None, true);
    fs::create_dir_all(&dir).unwrap();

    let path = dir.join(editor::QUERY_FILENAME);
    fs::write(&path, "the request this turn sent").unwrap();

    (fs, id, path, tmp)
}

/// A draft replaced while the turn ran belongs to whoever wrote it: a plugin
/// through `write_draft`, or another `jp` process.
/// A successful turn removes the request it sent, not text it never saw.
#[test]
fn cleanup_keeps_a_draft_written_after_the_request() {
    let (fs, id, path, _tmp) = draft_fixture();
    let at_send = draft_fingerprint(Some(&fs), &id);

    fs::write(&path, "the follow-up typed during the turn").unwrap();

    cleanup_query_message_file(
        Some(&fs),
        &id,
        DraftRemoval::IfUnchanged(at_send.as_deref()),
    );

    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "the follow-up typed during the turn"
    );
}

#[test]
fn cleanup_removes_the_draft_the_turn_sent() {
    let (fs, id, path, _tmp) = draft_fixture();
    let at_send = draft_fingerprint(Some(&fs), &id);

    cleanup_query_message_file(
        Some(&fs),
        &id,
        DraftRemoval::IfUnchanged(at_send.as_deref()),
    );

    assert!(!path.exists(), "the request that was sent is cleaned up");
}

/// An emptied editor buffer discards the draft it was seeded from, whatever it
/// now holds.
#[test]
fn cleanup_any_removes_a_replaced_draft() {
    let (fs, id, path, _tmp) = draft_fixture();

    fs::write(&path, "something else entirely").unwrap();

    cleanup_query_message_file(Some(&fs), &id, DraftRemoval::Any);

    assert!(!path.exists());
}
