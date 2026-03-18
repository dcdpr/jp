use std::fs;

use camino_tempfile::tempdir;
use test_log::test;

use super::*;
use crate::{CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE};

#[test]
fn test_moves_directory_and_writes_trashed_md() {
    let tmp = tempdir().unwrap();
    let conversations_dir = tmp.path().join(CONVERSATIONS_DIR);
    let dirname = "17457886043-my-chat";

    let conv_dir = conversations_dir.join(dirname);
    fs::create_dir_all(&conv_dir).unwrap();
    fs::write(conv_dir.join(METADATA_FILE), r#"{"test": true}"#).unwrap();
    fs::write(conv_dir.join(EVENTS_FILE), "[]").unwrap();

    let error_msg = format!("{METADATA_FILE}: expected value at line 3 column 1");
    trash_conversation(&conversations_dir, dirname, &error_msg).unwrap();

    // Original directory should be gone.
    assert!(!conv_dir.exists());

    // Files should be preserved in .trash/.
    let trashed = conversations_dir.join(TRASH_DIR).join(dirname);
    assert!(trashed.join(METADATA_FILE).is_file());
    assert!(trashed.join(EVENTS_FILE).is_file());

    // TRASHED.md should explain the error.
    let trashed_md = fs::read_to_string(trashed.join("TRASHED.md")).unwrap();
    assert!(trashed_md.contains("# Trashed Conversation"));
    assert!(trashed_md.contains(&error_msg));
    assert!(trashed_md.contains("**Date:**"));
}

#[test]
fn test_appends_suffix_on_name_collision() {
    let tmp = tempdir().unwrap();
    let conversations_dir = tmp.path().join(CONVERSATIONS_DIR);
    let dirname = "17457886043-my-chat";

    // Pre-create a trashed entry with the same name.
    let existing_trash = conversations_dir.join(TRASH_DIR).join(dirname);
    fs::create_dir_all(&existing_trash).unwrap();

    // Create the conversation to trash.
    let conv_dir = conversations_dir.join(dirname);
    fs::create_dir_all(&conv_dir).unwrap();
    fs::write(conv_dir.join(METADATA_FILE), "{}").unwrap();

    trash_conversation(&conversations_dir, dirname, "some error").unwrap();

    // Original should be gone.
    assert!(!conv_dir.exists());

    // Should land at {dirname}-1.
    let suffixed = conversations_dir
        .join(TRASH_DIR)
        .join(format!("{dirname}-1"));
    assert!(suffixed.exists());
    assert!(suffixed.join("TRASHED.md").is_file());

    // The pre-existing trash entry should still be there.
    assert!(existing_trash.exists());
}

#[test]
fn test_increments_suffix_past_existing_collisions() {
    let tmp = tempdir().unwrap();
    let conversations_dir = tmp.path().join(CONVERSATIONS_DIR);
    let dirname = "17457886043-my-chat";

    // Pre-create trashed entries for the base name and -1.
    let trash_base = conversations_dir.join(TRASH_DIR);
    fs::create_dir_all(trash_base.join(dirname)).unwrap();
    fs::create_dir_all(trash_base.join(format!("{dirname}-1"))).unwrap();

    let conv_dir = conversations_dir.join(dirname);
    fs::create_dir_all(&conv_dir).unwrap();
    fs::write(conv_dir.join(METADATA_FILE), "{}").unwrap();

    trash_conversation(&conversations_dir, dirname, "error").unwrap();

    // Should skip base and -1, land at -2.
    let target = trash_base.join(format!("{dirname}-2"));
    assert!(target.exists());
    assert!(target.join("TRASHED.md").is_file());
}

#[test]
fn test_nonexistent_source_returns_error() {
    let tmp = tempdir().unwrap();
    let conversations_dir = tmp.path().join(CONVERSATIONS_DIR);
    fs::create_dir_all(&conversations_dir).unwrap();

    let result = trash_conversation(&conversations_dir, "does-not-exist", "error");
    assert!(result.is_err());
}
