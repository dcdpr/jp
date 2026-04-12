use std::fs;

use camino_tempfile::tempdir;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use test_log::test;

use super::*;
use crate::{
    BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE, OLD_PREFIX, STAGING_PREFIX,
    Storage, value::write_json,
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

    trash_invalid_conversation(&result.invalid[0]).unwrap();

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
fn test_validation_cleans_up_orphaned_staging_dirs() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);

    // Simulate an orphaned staging directory from a crashed persist.
    let convs = tmp.path().join(CONVERSATIONS_DIR);
    let staging = convs.join(format!("{STAGING_PREFIX}{}", id.to_dirname(None)));
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("metadata.json"), "{}").unwrap();

    assert!(staging.exists());

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());

    // Staging directory should have been cleaned up.
    assert!(!staging.exists());
}

#[test]
fn test_validation_cleans_up_old_backup_dirs() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_valid(tmp.path(), &id);

    // Simulate a crash after step 5 failed: both the final dir and .old-
    // backup exist.
    let convs = tmp.path().join(CONVERSATIONS_DIR);
    let old = convs.join(format!("{OLD_PREFIX}{}", id.to_dirname(None)));
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("metadata.json"), "{}").unwrap();

    let result = storage.validate_conversations();
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());

    // Old backup should have been cleaned up.
    assert!(!old.exists());
}

#[test]
fn test_validation_rolls_back_interrupted_swap() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();

    // Simulate a crash between steps 3 and 4: the final dir was renamed to
    // .old-, the staging dir exists, but the final dir is gone.
    let convs = tmp.path().join(CONVERSATIONS_DIR);
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let dirname = id.to_dirname(None);

    let old = convs.join(format!("{OLD_PREFIX}{dirname}"));
    fs::create_dir_all(&old).unwrap();
    // Write a valid conversation in the .old- dir so it validates after rollback.
    write_json(&old.join(METADATA_FILE), &Conversation::default()).unwrap();
    let stream = ConversationStream::new_test();
    let (base_config, events) = stream.to_parts().unwrap();
    write_json(&old.join(BASE_CONFIG_FILE), &base_config).unwrap();
    write_json(&old.join(EVENTS_FILE), &events).unwrap();

    let staging = convs.join(format!("{STAGING_PREFIX}{dirname}"));
    fs::create_dir_all(&staging).unwrap();
    fs::write(staging.join("metadata.json"), "{}").unwrap();

    // Final dir does NOT exist.
    assert!(!convs.join(&dirname).exists());

    let result = storage.validate_conversations();

    // The .old- dir should have been rolled back to the final location.
    assert!(convs.join(&dirname).exists());
    assert!(!old.exists());
    assert!(!staging.exists());

    // The rolled-back conversation should validate.
    assert_eq!(result.valid.len(), 1);
    assert!(result.invalid.is_empty());
}
