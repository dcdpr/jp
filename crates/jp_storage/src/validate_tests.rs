use std::{
    fs,
    time::{Duration, SystemTime},
};

use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use test_log::test;

use super::*;
use crate::{
    BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE, Storage,
    scan::SCAN_CACHE_FILE, value::write_json,
};

const TWO_EVENTS: &str = r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request","content":"a"},{"timestamp":"2026-01-02T11:30:00Z","type":"chat_response","content":"b"}]"#;
const ONE_EVENT: &str =
    r#"[{"timestamp":"2026-03-04T05:06:07Z","type":"chat_request","content":"a"}]"#;

/// [`TWO_EVENTS`] with a later last timestamp and the same size in bytes.
const TWO_EVENTS_LATER: &str = r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request","content":"a"},{"timestamp":"2026-01-09T11:30:00Z","type":"chat_response","content":"b"}]"#;

/// Write a conversation with the given `events.json` body and no
/// `base_config.json`, returning its directory.
fn write_with_events(storage: &camino::Utf8Path, id: &ConversationId, events: &str) -> Utf8PathBuf {
    let dir = storage.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&dir).unwrap();
    write_json(&dir.join(METADATA_FILE), &Conversation::default()).unwrap();
    fs::write(dir.join(EVENTS_FILE), events).unwrap();
    dir
}

fn set_mtime(path: &camino::Utf8Path, when: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn set_dir_mtimes(dir: &camino::Utf8Path, when: SystemTime) {
    for name in [METADATA_FILE, EVENTS_FILE] {
        set_mtime(&dir.join(name), when);
    }
}

/// Overwrite `path` with spaces of the same byte length while preserving its
/// modification time.
fn blank_keeping_stamp(path: &camino::Utf8Path) {
    let meta = fs::metadata(path).unwrap();
    let modified = meta.modified().unwrap();
    fs::write(path, " ".repeat(usize::try_from(meta.len()).unwrap())).unwrap();
    set_mtime(path, modified);
}

/// Overwrite `path` with same-sized `content` while preserving its modification
/// time.
fn rewrite_keeping_stamp(path: &camino::Utf8Path, content: &str) {
    let meta = fs::metadata(path).unwrap();
    assert_eq!(u64::try_from(content.len()).unwrap(), meta.len());
    let modified = meta.modified().unwrap();
    fs::write(path, content).unwrap();
    set_mtime(path, modified);
}

fn an_hour_ago() -> SystemTime {
    SystemTime::now() - Duration::from_hours(1)
}

/// Write a valid conversation to disk under the given storage root.
fn write_valid(storage: &camino::Utf8Path, id: &ConversationId) {
    let dir = storage.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&dir).unwrap();
    write_json(&dir.join(METADATA_FILE), &Conversation::default()).unwrap();
    let stream = ConversationStream::new_test();
    let (base_config, events) = stream.to_parts().unwrap();
    write_json(&dir.join(BASE_CONFIG_FILE), &base_config).unwrap();
    write_json(&dir.join(EVENTS_FILE), &events).unwrap();
}

// `jp doctor` reports the store as it is, so diagnosing it lists what the
// startup scan would clean up, and changes nothing on disk.
#[test]
fn diagnosing_the_store_changes_nothing_on_disk() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");
    let storage = Storage::new(workspace.clone())
        .unwrap()
        .with_user_storage(&user_root, None, "abc")
        .unwrap();
    let cache = user_root
        .join("abc")
        .join(CONVERSATIONS_DIR)
        .join(SCAN_CACHE_FILE);
    fs::create_dir_all(cache.parent().unwrap()).unwrap();
    let conversations = workspace.join(CONVERSATIONS_DIR);

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(&workspace, &id);
    let tmp_file = conversations.join("17636257526").join("events.json.tmp");
    fs::write(&tmp_file, "partial").unwrap();
    let staging = conversations.join(crate::import_staging_dirname("17636257527"));
    fs::create_dir_all(&staging).unwrap();
    let broken = conversations.join("17636257528");
    fs::create_dir_all(&broken).unwrap();
    let notes = conversations.join("notes");
    fs::create_dir_all(&notes).unwrap();

    let diagnosis = storage.diagnose_conversations();

    let invalid: Vec<_> = diagnosis
        .invalid
        .iter()
        .map(InvalidConversation::path)
        .collect();
    assert_eq!(invalid, vec![broken]);
    assert_eq!(diagnosis.unrecognized, vec![notes]);
    assert_eq!(diagnosis.leftovers, vec![staging.clone(), tmp_file.clone()]);
    assert!(tmp_file.is_file());
    assert!(staging.is_dir());
    assert!(!cache.exists());
}

// A conversation whose events survive gets its metadata back: the title its
// directory name still carries, and the time of its last event.
#[test]
fn rebuilding_metadata_recovers_the_title_and_the_last_activity() {
    let tmp = tempdir().unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = tmp.path().join("17636257526-fix-the-loader");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(EVENTS_FILE), TWO_EVENTS).unwrap();

    assert!(missing_only_metadata(&dir));
    rebuild_metadata(&dir, &id).unwrap();

    let metadata: Conversation =
        serde_json::from_str(&fs::read_to_string(dir.join(METADATA_FILE)).unwrap()).unwrap();
    assert_eq!(metadata.title.as_deref(), Some("fix the loader"));
    assert_eq!(
        metadata.last_activated_at,
        Utc.with_ymd_and_hms(2026, 1, 2, 11, 30, 0).unwrap()
    );
    assert_eq!(validate_conversation_dir(&dir), Ok(()));
}

// Without a title in the directory name or an event to date it by, the
// conversation keeps no title and is dated by its ID.
#[test]
fn rebuilding_metadata_without_a_title_or_events_dates_it_by_its_id() {
    let tmp = tempdir().unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = tmp.path().join("17636257526");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(EVENTS_FILE), "[]").unwrap();

    rebuild_metadata(&dir, &id).unwrap();

    let metadata: Conversation =
        serde_json::from_str(&fs::read_to_string(dir.join(METADATA_FILE)).unwrap()).unwrap();
    assert_eq!(metadata.title, None);
    assert_eq!(metadata.last_activated_at, id.timestamp());
}

// Rebuilding the metadata only makes a conversation load when the metadata is
// all it is missing.
#[test]
fn only_a_conversation_missing_nothing_but_its_metadata_can_have_it_rebuilt() {
    let tmp = tempdir().unwrap();

    let no_events = tmp.path().join("17636257526");
    fs::create_dir_all(&no_events).unwrap();
    assert!(!missing_only_metadata(&no_events));

    let broken_events = tmp.path().join("17636257527");
    fs::create_dir_all(&broken_events).unwrap();
    fs::write(broken_events.join(EVENTS_FILE), "not json").unwrap();
    assert!(!missing_only_metadata(&broken_events));

    let broken_base_config = tmp.path().join("17636257528");
    fs::create_dir_all(&broken_base_config).unwrap();
    fs::write(broken_base_config.join(EVENTS_FILE), TWO_EVENTS).unwrap();
    fs::write(broken_base_config.join(BASE_CONFIG_FILE), "not json").unwrap();
    assert!(!missing_only_metadata(&broken_base_config));

    let has_metadata = tmp.path().join("17636257529");
    fs::create_dir_all(&has_metadata).unwrap();
    fs::write(has_metadata.join(METADATA_FILE), "not json").unwrap();
    fs::write(has_metadata.join(EVENTS_FILE), TWO_EVENTS).unwrap();
    assert!(!missing_only_metadata(&has_metadata));
}

#[test]
fn staging_dirs_are_swept_only_when_the_conversation_is_unlocked() {
    // The sweep runs at startup in every process, while another process may be
    // importing that same conversation under its lock. Reaping mid-copy would
    // let the importer rename an incomplete tree into place, so a held lock has
    // to make the sweep skip. Driven by taking the lock in-process rather than
    // racing two imports, so the outcome is deterministic.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let conversations = tmp.path().join(CONVERSATIONS_DIR);

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    // Built through the production helper: its per-attempt uniqueness suffix is
    // exactly what the sweep has to still recognise and be able to lock.
    let staging = conversations.join(crate::import_staging_dirname(&id.to_dirname(Some("title"))));
    fs::create_dir_all(staging.join("partial")).unwrap();

    let guard = storage
        .try_lock_conversation(&id.to_string(), None)
        .unwrap()
        .expect("the lock is free in this test");

    drop(storage.validate_conversations());
    assert!(
        staging.is_dir(),
        "an in-flight import's staging directory must survive the sweep"
    );

    drop(guard);

    drop(storage.validate_conversations());
    assert!(
        !staging.exists(),
        "an abandoned staging directory is reaped once nobody holds the lock"
    );
}

#[test]
fn the_sweep_leaves_real_conversations_and_dot_dirs_alone() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let conversations = tmp.path().join(CONVERSATIONS_DIR);

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);
    fs::create_dir_all(conversations.join(".archive")).unwrap();

    drop(storage.validate_conversations());

    assert!(conversations.join(id.to_dirname(None)).is_dir());
    assert!(conversations.join(".archive").is_dir());
}

#[test]
fn a_staging_dir_with_an_unparseable_name_is_left_in_place() {
    // Without a parseable id there is no lock to check, so removing it could
    // race an owner this code cannot identify.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let conversations = tmp.path().join(CONVERSATIONS_DIR);

    let staging = conversations.join(".import-not-an-id");
    fs::create_dir_all(&staging).unwrap();

    drop(storage.validate_conversations());

    assert!(staging.is_dir());
}

#[test]
fn test_valid_conversations_are_collected() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id1 = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let id2 = ConversationId::try_from_deciseconds_str("17636257527").unwrap();
    write_valid(tmp.path(), &id1);
    write_valid(tmp.path(), &id2);

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 2);
    assert!(result.invalid.is_empty());
}

// A directory whose name is not a conversation ID is not a conversation, so it
// cannot fail to load as one.
#[test]
fn a_directory_not_named_for_a_conversation_is_unrecognized() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);

    let bad = tmp.path().join(CONVERSATIONS_DIR).join("not-a-valid-id");
    fs::create_dir_all(&bad).unwrap();
    fs::write(bad.join(METADATA_FILE), "not json").unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());
    assert_eq!(result.unrecognized, [bad]);
}

#[test]
fn test_missing_metadata_is_detected() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&dir).unwrap();
    let stream = ConversationStream::new_test();
    let (base_config, events) = stream.to_parts().unwrap();
    write_json(&dir.join(BASE_CONFIG_FILE), &base_config).unwrap();
    write_json(&dir.join(EVENTS_FILE), &events).unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.invalid.len(), 1);
    assert_eq!(result.invalid[0].error, ValidationError::Missing {
        part: ConversationPart::Metadata,
        location: StorageLocation::new(dir.join(METADATA_FILE).as_str()),
    });
    assert_eq!(result.invalid[0].error.to_string(), "missing metadata.json");
}

#[test]
fn test_corrupt_metadata_is_detected() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(METADATA_FILE), "not json").unwrap();
    let stream = ConversationStream::new_test();
    let (base_config, events) = stream.to_parts().unwrap();
    write_json(&dir.join(BASE_CONFIG_FILE), &base_config).unwrap();
    write_json(&dir.join(EVENTS_FILE), &events).unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.invalid.len(), 1);
    assert_eq!(result.invalid[0].error, ValidationError::Unreadable {
        part: ConversationPart::Metadata,
        location: StorageLocation::new(dir.join(METADATA_FILE).as_str()),
        error: ReadError::Malformed {
            format: Some("JSON"),
            message: "expected ident at line 1 column 2".to_owned(),
            position: Some(Position { line: 1, column: 2 }),
        },
    });
    assert_eq!(
        result.invalid[0].error.to_string(),
        "metadata.json: invalid JSON at line 1, column 2"
    );
}

// A backend names the format its parts are in; one whose parts have no named
// format still says what is wrong.
#[test]
fn a_malformed_part_is_named_by_its_format() {
    assert_eq!(malformed(Some("JSON")), "invalid JSON");
    assert_eq!(malformed(None), "malformed");
}

#[test]
fn test_missing_events_is_detected() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&dir).unwrap();
    write_json(&dir.join(METADATA_FILE), &Conversation::default()).unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.invalid.len(), 1);
    assert_eq!(result.invalid[0].error, ValidationError::Missing {
        part: ConversationPart::Events,
        location: StorageLocation::new(dir.join(EVENTS_FILE).as_str()),
    });
    assert_eq!(result.invalid[0].error.to_string(), "missing events.json");
}

#[test]
fn test_corrupt_events_is_detected() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&dir).unwrap();
    write_json(&dir.join(METADATA_FILE), &Conversation::default()).unwrap();
    fs::write(dir.join(EVENTS_FILE), "not json").unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.invalid.len(), 1);
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::Unreadable {
            part: ConversationPart::Events,
            error: ReadError::Malformed {
                position: Some(Position { line: 1, column: 2 }),
                ..
            },
            ..
        }
    ));
    assert_eq!(
        result.invalid[0].error.to_string(),
        "events.json: invalid JSON at line 1, column 2"
    );
}

// A conversation is stored before anything else is written into its directory,
// so a directory without the conversation's files is missing them, whatever
// else it holds.
#[test]
fn a_directory_without_conversation_files_is_invalid() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let convs = tmp.path().join(CONVERSATIONS_DIR);

    let drafted = convs.join("17636257526");
    fs::create_dir_all(&drafted).unwrap();
    fs::write(drafted.join("QUERY_MESSAGE.md"), "typed request").unwrap();

    let empty = convs.join("17636257527-a-title");
    fs::create_dir_all(&empty).unwrap();

    let result = storage.validate_conversations();
    assert!(result.valid.is_empty());

    let mut invalid: Vec<_> = result
        .invalid
        .iter()
        .map(|entry| (entry.path(), entry.error.to_string()))
        .collect();
    invalid.sort();
    assert_eq!(invalid, [
        (drafted, "missing metadata.json".to_owned()),
        (empty, "missing metadata.json".to_owned()),
    ]);
}

// One directory, wherever it lives, gets the checks a directory under
// `conversations/` gets at startup.
#[test]
fn a_single_directory_is_validated_outside_the_conversations_dir() {
    let tmp = tempdir().unwrap();
    let elsewhere = tmp.path().join("elsewhere");

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(&elsewhere, &id);
    let stored = elsewhere.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    assert_eq!(validate_conversation_dir(&stored), Ok(()));

    let empty = tmp.path().join("17636257529");
    fs::create_dir_all(&empty).unwrap();
    assert!(matches!(
        validate_conversation_dir(&empty),
        Err(ValidationError::Missing {
            part: ConversationPart::Metadata,
            ..
        })
    ));

    // The checks are on the files alone, whatever the directory is named.
    let missing_metadata = tmp.path().join("renamed");
    fs::create_dir_all(&missing_metadata).unwrap();
    fs::write(missing_metadata.join(EVENTS_FILE), "[]").unwrap();
    assert!(matches!(
        validate_conversation_dir(&missing_metadata),
        Err(ValidationError::Missing {
            part: ConversationPart::Metadata,
            ..
        })
    ));
}

#[test]
fn test_skips_files_and_dot_dirs() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);

    let convs = tmp.path().join(CONVERSATIONS_DIR);
    fs::create_dir_all(convs.join(".trash")).unwrap();
    fs::create_dir_all(convs.join(".hidden")).unwrap();
    fs::write(convs.join("some-file.txt"), "").unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());
}

#[test]
fn test_empty_conversations_dir() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    fs::create_dir_all(tmp.path().join(CONVERSATIONS_DIR)).unwrap();

    let result = storage.validate_conversations();
    assert!(result.valid.is_empty());
    assert!(result.invalid.is_empty());
}

#[test]
fn test_no_conversations_dir() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let result = storage.validate_conversations();
    assert!(result.valid.is_empty());
    assert!(result.invalid.is_empty());
}

#[test]
fn test_validation_cleans_up_orphaned_tmp_files() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);

    // Simulate orphaned .tmp files from a crashed atomic write.
    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::write(conv_dir.join("metadata.json.tmp"), "partial").unwrap();
    fs::write(conv_dir.join("events.json.tmp"), "partial").unwrap();

    assert!(conv_dir.join("metadata.json.tmp").exists());
    assert!(conv_dir.join("events.json.tmp").exists());

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());

    // .tmp files should have been cleaned up.
    assert!(!conv_dir.join("metadata.json.tmp").exists());
    assert!(!conv_dir.join("events.json.tmp").exists());

    // Real files should still be there.
    assert!(conv_dir.join("metadata.json").exists());
    assert!(conv_dir.join("events.json").exists());
}

#[test]
fn tmp_files_are_swept_only_when_the_conversation_is_unlocked() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);
    let in_flight = tmp
        .path()
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None))
        .join("events.json.tmp");
    fs::write(&in_flight, "partial").unwrap();

    let guard = storage
        .try_lock_conversation(&id.to_string(), None)
        .unwrap()
        .expect("the lock is free in this test");

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(
        in_flight.is_file(),
        "the lock holder's in-flight write must survive the sweep"
    );

    drop(guard);

    drop(storage.validate_conversations());
    assert!(
        !in_flight.exists(),
        "an orphaned temp file is reaped once nobody holds the lock"
    );
}

#[test]
fn an_unchanged_conversation_is_not_parsed_again() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(tmp.path(), &id, TWO_EVENTS);
    set_dir_mtimes(&dir, an_hour_ago());

    assert_eq!(storage.validate_conversations().valid.len(), 1);

    // Same size and modification time, so validation uses the cached scan
    // without re-reading the blanked file.
    blank_keeping_stamp(&dir.join(EVENTS_FILE));

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());
}

#[test]
fn a_rewritten_events_file_is_validated_again() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(tmp.path(), &id, TWO_EVENTS);
    set_dir_mtimes(&dir, an_hour_ago());

    assert_eq!(storage.validate_conversations().valid.len(), 1);

    fs::write(dir.join(EVENTS_FILE), r#"[{"broken": true}]"#).unwrap();

    let result = storage.validate_conversations();
    assert!(result.valid.is_empty());
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::Unreadable {
            part: ConversationPart::Events,
            ..
        }
    ));
}

#[test]
fn tmp_files_are_cleaned_up_for_an_unchanged_conversation() {
    // A crash between writing `events.json.tmp` and the rename leaves
    // `events.json` untouched, so the sweep must not depend on the cache.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(tmp.path(), &id, TWO_EVENTS);
    set_dir_mtimes(&dir, an_hour_ago());

    assert_eq!(storage.validate_conversations().valid.len(), 1);
    fs::write(dir.join("events.json.tmp"), "partial").unwrap();

    assert_eq!(storage.validate_conversations().valid.len(), 1);
    assert!(!dir.join("events.json.tmp").exists());
}

#[test]
fn the_scan_is_reused_by_the_next_process() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");
    let open = || {
        Storage::new(workspace.clone())
            .unwrap()
            .with_user_storage(&user_root, None, "abc")
            .unwrap()
    };
    let user_dir = user_root.join("abc");
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(&user_dir, &id, TWO_EVENTS);
    set_dir_mtimes(&dir, an_hour_ago());

    assert_eq!(open().validate_conversations().valid.len(), 1);
    assert!(
        user_dir
            .join(CONVERSATIONS_DIR)
            .join(SCAN_CACHE_FILE)
            .is_file()
    );

    blank_keeping_stamp(&dir.join(EVENTS_FILE));

    let result = open().validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());
}

#[test]
fn a_scan_of_a_just_modified_file_is_not_reused_by_the_next_process() {
    // Files modified within `MTIME_GRANULARITY` of the scan cannot be
    // distinguished from a same-tick rewrite, so they are not persisted to the
    // scan cache. Setting mtime in the future guarantees it falls within the
    // window.
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");
    let open = || {
        Storage::new(workspace.clone())
            .unwrap()
            .with_user_storage(&user_root, None, "abc")
            .unwrap()
    };
    let user_dir = user_root.join("abc");
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(&user_dir, &id, TWO_EVENTS);
    set_dir_mtimes(&dir, SystemTime::now() + Duration::from_hours(1));

    assert_eq!(open().validate_conversations().valid.len(), 1);

    blank_keeping_stamp(&dir.join(EVENTS_FILE));

    let result = open().validate_conversations();
    assert!(result.valid.is_empty());
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::Unreadable {
            part: ConversationPart::Events,
            ..
        }
    ));
}

#[test]
fn a_same_stamp_rewrite_just_after_a_scan_is_read_again() {
    // On a filesystem with coarse timestamps, a same-size rewrite in the same
    // clock tick preserves both size and modification time. A scan that ran
    // one second after the write must not be cached.
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(tmp.path(), &id, TWO_EVENTS);
    let written = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_dir_mtimes(&dir, written);

    let scan_started = written + Duration::from_secs(1);
    assert_eq!(
        storage.validate_conversations_at(scan_started).valid.len(),
        1
    );

    rewrite_keeping_stamp(&dir.join(EVENTS_FILE), TWO_EVENTS_LATER);

    let conversation = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conversation.last_event_at,
        Some(Utc.with_ymd_and_hms(2026, 1, 9, 11, 30, 0).unwrap())
    );

    blank_keeping_stamp(&dir.join(EVENTS_FILE));

    let result = storage.validate_conversations_at(scan_started);
    assert!(result.valid.is_empty());
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::Unreadable {
            part: ConversationPart::Events,
            ..
        }
    ));
}

#[test]
fn a_scan_taken_just_after_a_write_is_not_persisted_by_a_later_validation() {
    // When the same `Storage` instance validates again after the granularity
    // window has passed, it must re-parse the rewritten file rather than
    // persisting the earlier uncached scan.
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");
    let open = || {
        Storage::new(workspace.clone())
            .unwrap()
            .with_user_storage(&user_root, None, "abc")
            .unwrap()
    };
    let user_dir = user_root.join("abc");
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(&user_dir, &id, TWO_EVENTS);
    let written = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    set_dir_mtimes(&dir, written);

    let storage = open();
    let just_after = written + Duration::from_secs(1);
    assert_eq!(storage.validate_conversations_at(just_after).valid.len(), 1);

    rewrite_keeping_stamp(&dir.join(EVENTS_FILE), TWO_EVENTS_LATER);

    let later = written + Duration::from_hours(1);
    assert_eq!(storage.validate_conversations_at(later).valid.len(), 1);

    let fresh = open();
    assert_eq!(fresh.validate_conversations_at(later).valid.len(), 1);
    let conversation = fresh.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conversation.last_event_at,
        Some(Utc.with_ymd_and_hms(2026, 1, 9, 11, 30, 0).unwrap())
    );
}

#[test]
fn metadata_loads_take_the_event_summary_from_the_scan() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(tmp.path(), &id, TWO_EVENTS);
    set_dir_mtimes(&dir, an_hour_ago());

    assert_eq!(storage.validate_conversations().valid.len(), 1);
    blank_keeping_stamp(&dir.join(EVENTS_FILE));

    let expected_last = Some(Utc.with_ymd_and_hms(2026, 1, 2, 11, 30, 0).unwrap());

    let single = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(single.events_count, 2);
    assert_eq!(single.last_event_at, expected_last);

    let batch = storage.load_conversation_metadata_batch(&[id]);
    let batched = batch[0].1.as_ref().unwrap();
    assert_eq!(batched.events_count, 2);
    assert_eq!(batched.last_event_at, expected_last);
}

#[test]
fn metadata_loads_count_a_rewritten_events_file_again() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dir = write_with_events(tmp.path(), &id, TWO_EVENTS);
    set_dir_mtimes(&dir, an_hour_ago());

    assert_eq!(storage.validate_conversations().valid.len(), 1);
    fs::write(dir.join(EVENTS_FILE), ONE_EVENT).unwrap();

    let expected_last = Some(Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap());

    let single = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(single.events_count, 1);
    assert_eq!(single.last_event_at, expected_last);

    let batch = storage.load_conversation_metadata_batch(&[id]);
    let batched = batch[0].1.as_ref().unwrap();
    assert_eq!(batched.events_count, 1);
    assert_eq!(batched.last_event_at, expected_last);
}
