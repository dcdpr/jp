use std::{
    fs::{self, File},
    str::FromStr as _,
};

use camino_tempfile::tempdir;
use chrono::TimeZone as _;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use serde_json::json;
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
fn test_remove_ephemeral_conversations() {
    let storage_dir = tempdir().unwrap();
    let path = storage_dir.path();
    let convs = path.join(CONVERSATIONS_DIR);

    let id1 = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let id2 = ConversationId::try_from_deciseconds_str("17636257527").unwrap();
    let id3 = ConversationId::try_from_deciseconds_str("17636257528").unwrap();
    let id4 = ConversationId::try_from_deciseconds_str("17636257529").unwrap();
    let id5 = ConversationId::try_from_deciseconds_str("17636257530").unwrap();

    let dir1 = convs.join(id1.to_dirname(None));
    fs::create_dir_all(&dir1).unwrap();
    write_json(
        &dir1.join(METADATA_FILE),
        &json!({
            "last_activated_at": Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            "expires_at": Utc::now() - chrono::Duration::hours(1)
        }),
    )
    .unwrap();
    write_json(&dir1.join(EVENTS_FILE), &json!([])).unwrap();

    let title = "hello world";
    let dir2 = convs.join(id2.to_dirname(Some(title)));
    fs::create_dir_all(&dir2).unwrap();
    write_json(
        &dir2.join(METADATA_FILE),
        &json!({
            "title": title,
            "last_activated_at": Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            "expires_at": Utc::now() + chrono::Duration::hours(1)
        }),
    )
    .unwrap();
    write_json(&dir2.join(EVENTS_FILE), &json!([])).unwrap();

    let dir3 = convs.join(id3.to_dirname(Some(title)));
    fs::create_dir_all(&dir3).unwrap();
    write_json(
        &dir3.join(METADATA_FILE),
        &json!({
            "title": title,
            "last_activated_at": Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
            "expires_at": Utc::now() - chrono::Duration::hours(1)
        }),
    )
    .unwrap();
    write_json(&dir3.join(EVENTS_FILE), &json!([])).unwrap();

    fs::create_dir_all(convs.join(id4.to_dirname(None))).unwrap();
    fs::create_dir_all(convs.join(id5.to_dirname(Some("foo")))).unwrap();

    let storage = Storage::new(path).unwrap();
    storage.remove_ephemeral_conversations(&[id4, id5]);

    assert!(!convs.join(id1.to_dirname(None)).exists());
    assert!(convs.join(id2.to_dirname(Some(title))).exists());
    assert!(!convs.join(id3.to_dirname(Some(title))).exists());
    assert!(convs.join(id4.to_dirname(None)).exists());
    assert!(convs.join(id5.to_dirname(Some("foo"))).exists());
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

    // Create the conversations dir, then make it read-only so the staging
    // dir creation fails.
    let convs = tmp.path().join(CONVERSATIONS_DIR);
    fs::create_dir_all(&convs).unwrap();
    let mut perms = fs::metadata(&convs).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(true);
    fs::set_permissions(&convs, perms.clone()).unwrap();

    let result = storage.persist_conversation(&id, &metadata, &events);
    assert!(result.is_err());

    // Restore permissions for cleanup + assertions.
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&convs, perms).unwrap();

    // No conversation dir and no staging dir should exist.
    let conv_dir = convs.join(id.to_dirname(None));
    assert!(
        !conv_dir.exists(),
        "conversation dir should not exist after failed write"
    );

    let has_staging = fs::read_dir(&convs)
        .unwrap()
        .flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(STAGING_PREFIX));
    assert!(!has_staging, "staging dir should be cleaned up on failure");
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

    // Make the conversations dir read-only so staging dir creation fails.
    let convs = tmp.path().join(CONVERSATIONS_DIR);
    let mut perms = fs::metadata(&convs).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(true);
    fs::set_permissions(&convs, perms.clone()).unwrap();

    let result = storage.persist_conversation(&id, &metadata, &events);
    assert!(result.is_err());

    // Restore permissions.
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&convs, perms).unwrap();

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
