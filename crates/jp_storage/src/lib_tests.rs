use std::{
    fs::{self, File},
    str::FromStr as _,
};

use camino_tempfile::tempdir;
use chrono::TimeZone as _;
use jp_conversation::ConversationId;
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
