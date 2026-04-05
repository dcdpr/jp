use std::fs;

use camino_tempfile::tempdir;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use test_log::test;

use super::*;
use crate::{
    BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE, Storage, value::write_json,
};

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

#[test]
fn test_invalid_dirname_is_detected() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);

    let bad = tmp.path().join(CONVERSATIONS_DIR).join("not-a-valid-id");
    fs::create_dir_all(&bad).unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert_eq!(result.invalid.len(), 1);
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::InvalidDirname
    ));
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
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::MissingMetadata
    ));
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
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::CorruptMetadata { .. }
    ));
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
    assert!(matches!(
        result.invalid[0].error,
        ValidationError::MissingEvents
    ));
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
        ValidationError::CorruptEvents { .. }
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
fn test_trash_moves_invalid_to_trash() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);

    // Create an invalid entry.
    let bad = tmp.path().join(CONVERSATIONS_DIR).join("not-valid");
    fs::create_dir_all(&bad).unwrap();
    fs::write(bad.join("somefile"), "data").unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.invalid.len(), 1);

    storage.trash_conversation(&result.invalid[0]).unwrap();

    // Original should be gone, trash should have it.
    assert!(!bad.exists());
    assert!(
        tmp.path()
            .join(CONVERSATIONS_DIR)
            .join(".trash")
            .join("not-valid")
            .exists()
    );
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
