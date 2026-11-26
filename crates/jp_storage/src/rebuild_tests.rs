use std::{fs, time::SystemTime};

use camino::Utf8Path;
use camino_tempfile::tempdir;
use chrono::{DateTime, TimeZone as _, Utc};
use jp_conversation::{Conversation, ConversationId};
use test_log::test;

use super::*;
use crate::{
    EVENTS_FILE, METADATA_FILE,
    validate::{missing_only_metadata, validate_conversation_dir},
};

const TWO_EVENTS: &str = r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request","content":"a"},{"timestamp":"2026-01-02T11:30:00Z","type":"chat_response","content":"b"}]"#;

fn id() -> ConversationId {
    ConversationId::try_from_deciseconds_str("17636257526").unwrap()
}

fn at(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap()
}

fn read_metadata(dir: &Utf8Path) -> Conversation {
    serde_json::from_str(&fs::read_to_string(dir.join(METADATA_FILE)).unwrap()).unwrap()
}

#[test]
fn a_live_conversation_gets_its_title_and_last_activity_and_no_archive_time() {
    let metadata = rebuilt_metadata(&Remains {
        id: id(),
        dirname: "17636257526-fix-the-loader",
        last_event_at: Some(at(2026, 1, 2, 11)),
        modified_at: Some(at(2026, 3, 4, 5)),
        archived: false,
    });

    assert_eq!(metadata.title.as_deref(), Some("fix the loader"));
    assert_eq!(metadata.last_activated_at, at(2026, 1, 2, 11));
    assert_eq!(metadata.archived_at, None);
}

#[test]
fn an_archived_conversation_is_dated_by_its_directory() {
    let metadata = rebuilt_metadata(&Remains {
        id: id(),
        dirname: "17636257526",
        last_event_at: None,
        modified_at: Some(at(2026, 3, 4, 5)),
        archived: true,
    });

    assert_eq!(metadata.title, None);
    assert_eq!(metadata.last_activated_at, id().timestamp());
    assert_eq!(metadata.archived_at, Some(at(2026, 3, 4, 5)));
}

#[test]
fn rebuilding_metadata_recovers_the_title_and_the_last_activity() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("17636257526-fix-the-loader");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(EVENTS_FILE), TWO_EVENTS).unwrap();

    assert!(missing_only_metadata(&dir));
    rebuild_metadata(&dir, &id(), false).unwrap();

    let metadata = read_metadata(&dir);
    assert_eq!(metadata.title.as_deref(), Some("fix the loader"));
    assert_eq!(
        metadata.last_activated_at,
        Utc.with_ymd_and_hms(2026, 1, 2, 11, 30, 0).unwrap()
    );
    assert_eq!(metadata.archived_at, None);
    assert_eq!(validate_conversation_dir(&dir), Ok(()));
}

#[test]
fn rebuilding_an_archived_conversation_dates_it_by_its_directory() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("17636257526");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(EVENTS_FILE), "[]").unwrap();
    set_dir_modified(&dir, SystemTime::from(at(2026, 3, 4, 5)));

    rebuild_metadata(&dir, &id(), true).unwrap();

    assert_eq!(read_metadata(&dir).archived_at, Some(at(2026, 3, 4, 5)));
}

#[cfg(not(windows))]
fn set_dir_modified(dir: &Utf8Path, when: SystemTime) {
    fs::File::open(dir).unwrap().set_modified(when).unwrap();
}

// On Windows, opening a directory handle to modify its timestamps requires
// `FILE_FLAG_BACKUP_SEMANTICS` and `FILE_WRITE_ATTRIBUTES`.
#[cfg(windows)]
fn set_dir_modified(dir: &Utf8Path, when: SystemTime) {
    use std::os::windows::fs::OpenOptionsExt as _;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_WRITE_ATTRIBUTES,
    };

    fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)
        .unwrap()
        .set_modified(when)
        .unwrap();
}
