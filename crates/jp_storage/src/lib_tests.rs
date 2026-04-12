use std::{
    fs::{self, File},
    str::FromStr as _,
};

use camino_tempfile::tempdir;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use test_log::test;

use super::*;

#[test]
fn test_storage_handles_missing_src() {
    let missing_path = Utf8PathBuf::from("./non_existent_jp_workspace_source_dir_abc123");
    assert!(!missing_path.exists());

    let storage = Storage::new(&missing_path).expect("must succeed");
    assert!(storage.root.is_dir());
    assert_eq!(fs::read_dir(&storage.root).unwrap().count(), 0);
    assert_eq!(storage.root, missing_path);

    fs::remove_dir_all(&missing_path).ok();
}

#[test]
fn test_storage_new_errors_on_source_file() {
    let source_dir = tempdir().unwrap();
    let source_file_path = source_dir.path().join("source_is_a_file.txt");
    File::create(&source_file_path).unwrap();

    let result = Storage::new(&source_file_path);
    match result.expect_err("must fail") {
        Error::NotDir(path) => assert_eq!(path, source_file_path),
        _ => panic!("Expected Error::SourceNotDir"),
    }
}

#[test]
fn test_conversation_dir_name_generation() {
    let id = ConversationId::from_str("jp-c17457886043-otvo8").unwrap();
    assert_eq!(id.to_dirname(None), "17457886043");
    assert_eq!(
        id.to_dirname(Some("Simple Title")),
        "17457886043-simple-title"
    );
    assert_eq!(
        id.to_dirname(Some(" Title with spaces & chars!")),
        "17457886043-title-with-spaces---chars" // Sanitized
    );
    assert_eq!(
        id.to_dirname(Some(
            "A very long title that definitely exceeds the sixty character limit for testing \
             purposes"
        )),
        "17457886043-a-very-long-title-that-definitely-exceeds-the-sixty" // Truncated
    );
    assert_eq!(
        id.to_dirname(Some("")), // Empty title
        "17457886043"
    );
}

#[test]
fn load_all_conversation_ids_waits_for_inflight_persist() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let convs = tmp.path().join(CONVERSATIONS_DIR);

    // Simulate mid-persist: only the `.old-*` directory exists.
    let old_dir = convs.join(format!("{OLD_PREFIX}{}", id.to_dirname(None)));
    fs::create_dir_all(&old_dir).unwrap();

    // Spawn a thread that "completes" the persist after a brief delay
    // by creating the normal directory.
    let convs_clone = convs.clone();
    let id_clone = id;
    let handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(3));
        fs::create_dir_all(convs_clone.join(id_clone.to_dirname(None))).unwrap();
    });

    let ids = storage.load_all_conversation_ids();
    handle.join().unwrap();

    assert!(
        ids.contains(&id),
        "scan should find the conversation after retry"
    );
}

#[test]
fn load_all_conversation_ids_skips_orphaned_inflight_dir() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let convs = tmp.path().join(CONVERSATIONS_DIR);

    // Only a `.staging-*` dir exists, no normal dir ever appears (crashed
    // persist). The scan retries but ultimately does not return the ID.
    let staging_dir = convs.join(format!("{STAGING_PREFIX}{}", id.to_dirname(None)));
    fs::create_dir_all(&staging_dir).unwrap();

    let ids = storage.load_all_conversation_ids();
    assert!(
        !ids.contains(&id),
        "orphaned in-flight dir should not produce an ID"
    );
}

#[test]
fn load_all_conversation_ids_ignores_inflight_when_normal_exists() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let convs = tmp.path().join(CONVERSATIONS_DIR);

    // Both the final directory and a leftover .old- exist (crash after swap
    // but before cleanup). Should return the ID exactly once.
    fs::create_dir_all(convs.join(id.to_dirname(None))).unwrap();
    fs::create_dir_all(convs.join(format!("{OLD_PREFIX}{}", id.to_dirname(None)))).unwrap();

    let ids = storage.load_all_conversation_ids();
    assert_eq!(
        ids.iter().filter(|i| **i == id).count(),
        1,
        "should appear exactly once"
    );
}

#[test]
fn load_all_conversation_ids_still_skips_trash() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let convs = tmp.path().join(CONVERSATIONS_DIR);

    fs::create_dir_all(convs.join(".trash")).unwrap();

    let ids = storage.load_all_conversation_ids();
    assert!(ids.is_empty(), ".trash should still be invisible to scan");
}

#[test]
fn test_persist_conversation_creates_all_files() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let metadata = Conversation::default();
    let events = ConversationStream::new_test();

    storage
        .persist_conversation(&id, &metadata, &events)
        .unwrap();

    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    assert!(conv_dir.join(METADATA_FILE).is_file());
    assert!(conv_dir.join(BASE_CONFIG_FILE).is_file());
    assert!(conv_dir.join(EVENTS_FILE).is_file());

    // No staging dir should remain.
    let has_staging = fs::read_dir(tmp.path().join(CONVERSATIONS_DIR))
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(STAGING_PREFIX));
    assert!(
        !has_staging,
        "no staging directory should remain after success"
    );
}

#[test]
fn test_persist_conversation_no_dir_on_write_failure() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let metadata = Conversation::default();
    let events = ConversationStream::new_test();

    // Place a file where the staging directory would be. remove_dir_all fails
    // on a file (not a directory) on all platforms, causing an early error
    // before any conversation data is written.
    let convs = tmp.path().join(CONVERSATIONS_DIR);
    fs::create_dir_all(&convs).unwrap();
    let blocker = convs.join(format!("{STAGING_PREFIX}{}", id.to_dirname(None)));
    fs::write(&blocker, "not a directory").unwrap();

    let result = storage.persist_conversation(&id, &metadata, &events);
    assert!(result.is_err());

    // Clean up blocker so we can check for real artifacts.
    fs::remove_file(&blocker).unwrap();

    let conv_dir = convs.join(id.to_dirname(None));
    assert!(
        !conv_dir.exists(),
        "conversation dir should not exist after failed write"
    );
}

#[test]
fn test_persist_conversation_preserves_existing_base_config() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let metadata = Conversation::default();
    let events = ConversationStream::new_test();

    // First persist creates base_config.json.
    storage
        .persist_conversation(&id, &metadata, &events)
        .unwrap();

    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let base_config_path = conv_dir.join(BASE_CONFIG_FILE);

    // Simulate a user editing base_config.json.
    let original_content = fs::read_to_string(&base_config_path).unwrap();
    let marker = format!("{original_content}{{\"user_edit\": true}}");
    fs::write(&base_config_path, &marker).unwrap();

    // Second persist should preserve the user-edited base_config.json.
    storage
        .persist_conversation(&id, &metadata, &events)
        .unwrap();

    let after = fs::read_to_string(&base_config_path).unwrap();
    assert_eq!(
        after, marker,
        "base_config.json should be preserved across persists"
    );
}

#[test]
fn test_persist_conversation_preserves_non_managed_files() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let metadata = Conversation::default();
    let events = ConversationStream::new_test();

    // First persist.
    storage
        .persist_conversation(&id, &metadata, &events)
        .unwrap();

    // Add a non-managed file (like QUERY_MESSAGE.md from the editor).
    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let extra_file = conv_dir.join("QUERY_MESSAGE.md");
    fs::write(&extra_file, "user query content").unwrap();

    // Second persist should not destroy the extra file.
    storage
        .persist_conversation(&id, &metadata, &events)
        .unwrap();

    assert!(extra_file.is_file());
    assert_eq!(
        fs::read_to_string(&extra_file).unwrap(),
        "user query content"
    );
}

#[test]
fn test_persist_conversation_existing_survives_failed_update() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let metadata = Conversation::default();
    let events = ConversationStream::new_test();

    // Initial persist.
    storage
        .persist_conversation(&id, &metadata, &events)
        .unwrap();

    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let original_metadata = fs::read_to_string(conv_dir.join(METADATA_FILE)).unwrap();
    let original_events = fs::read_to_string(conv_dir.join(EVENTS_FILE)).unwrap();

    // Place a file where the staging directory would be created.
    let convs = tmp.path().join(CONVERSATIONS_DIR);
    let blocker = convs.join(format!("{STAGING_PREFIX}{}", id.to_dirname(None)));
    fs::write(&blocker, "not a directory").unwrap();

    let result = storage.persist_conversation(&id, &metadata, &events);
    assert!(result.is_err());

    // Clean up blocker.
    fs::remove_file(&blocker).unwrap();

    // Original conversation data should be intact.
    assert_eq!(
        fs::read_to_string(conv_dir.join(METADATA_FILE)).unwrap(),
        original_metadata
    );
    assert_eq!(
        fs::read_to_string(conv_dir.join(EVENTS_FILE)).unwrap(),
        original_events
    );
}
