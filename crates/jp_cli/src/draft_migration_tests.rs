use std::{fs, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_config::{AppConfig, PartialAppConfig};
use jp_conversation::ConversationId;
use jp_storage::{
    backend::{FsStorageBackend, LoadBackend as _, LockBackend as _},
    validate::{DirContents, validate_conversation_dir},
};

use super::*;

fn setup() -> (Utf8TempDir, FsStorageBackend, Utf8PathBuf, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let fs = FsStorageBackend::new(&tmp.path().join(".jp"))
        .unwrap()
        .with_user_storage(&tmp.path().join("user"), None, "abc")
        .unwrap();
    let [workspace, user] = <[Utf8PathBuf; 2]>::try_from(fs.conversations_dirs()).unwrap();
    (tmp, fs, workspace, user)
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

fn write_dir(conversations: &Utf8Path, dirname: &str, files: &[(&str, &str)]) -> Utf8PathBuf {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        fs::write(dir.join(name), content).unwrap();
    }
    dir
}

fn migrate(fs: &FsStorageBackend) {
    let unstored = fs.sanitize().unwrap().unstored;
    store_draft_only_conversations(fs, &unstored, || Some(Arc::new(AppConfig::new_test())));
}

// A draft older versions left without its conversation becomes a conversation
// with no turns, created when its ID says, holding the draft it had.
#[test]
fn a_draft_only_directory_becomes_a_conversation() {
    let (_tmp, fs, workspace, user) = setup();
    let dir = write_dir(&user, "17941219991", &[("QUERY_MESSAGE.md", "typed")]);

    migrate(&fs);

    assert!(matches!(
        validate_conversation_dir(&dir),
        Ok(DirContents::Stored)
    ));
    assert_eq!(
        fs::read_to_string(dir.join("QUERY_MESSAGE.md")).unwrap(),
        "typed"
    );

    let metadata = fs.load_conversation_metadata(&id("17941219991")).unwrap();
    assert_eq!(metadata.last_activated_at, id("17941219991").timestamp());
    assert_eq!(metadata.title, None);
    let stream = fs
        .load_conversation_stream(&id("17941219991"), &PartialAppConfig::empty())
        .unwrap();
    assert_eq!(stream.turn_count(), 0);

    // Kept where it was found: user-local, out of the committed tree.
    assert!(!workspace.join("17941219991").exists());
}

// Without user-local storage the draft sat in the workspace, and the
// conversation is stored there.
#[test]
fn a_draft_only_directory_in_the_workspace_is_stored_there() {
    let tmp = tempdir().unwrap();
    let fs = FsStorageBackend::new(&tmp.path().join(".jp")).unwrap();
    let workspace = tmp.path().join(".jp").join("conversations");
    let dir = write_dir(&workspace, "17941219991", &[("QUERY_MESSAGE.md", "typed")]);

    migrate(&fs);

    assert!(matches!(
        validate_conversation_dir(&dir),
        Ok(DirContents::Stored)
    ));
    assert_eq!(
        fs::read_to_string(dir.join("QUERY_MESSAGE.md")).unwrap(),
        "typed"
    );
}

// Nothing a user wrote is in an empty directory, so there is nothing to keep.
#[test]
fn an_empty_directory_is_left_alone() {
    let (_tmp, fs, _workspace, user) = setup();
    let dir = write_dir(&user, "17941219991", &[]);

    migrate(&fs);

    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
}

// Writing would reconcile the ID to one directory, deleting the other.
#[test]
fn an_id_with_a_directory_in_each_root_is_left_alone() {
    let (_tmp, fs, workspace, user) = setup();
    let local = write_dir(&user, "17941219991", &[("QUERY_MESSAGE.md", "typed")]);
    let shared = write_dir(&workspace, "17941219991", &[("notes", "kept")]);

    migrate(&fs);

    assert!(matches!(
        validate_conversation_dir(&local),
        Ok(DirContents::Unstored)
    ));
    assert_eq!(fs::read_to_string(shared.join("notes")).unwrap(), "kept");
}

#[test]
fn a_locked_conversation_is_left_for_a_later_run() {
    let (_tmp, fs, _workspace, user) = setup();
    let dir = write_dir(&user, "17941219991", &[("QUERY_MESSAGE.md", "typed")]);
    let unstored = fs.sanitize().unwrap().unstored;

    let held = fs
        .try_lock(&id("17941219991").to_string(), None)
        .unwrap()
        .unwrap();
    store_draft_only_conversations(&fs, &unstored, || Some(Arc::new(AppConfig::new_test())));
    assert!(matches!(
        validate_conversation_dir(&dir),
        Ok(DirContents::Unstored)
    ));

    drop(held);
    migrate(&fs);
    assert!(matches!(
        validate_conversation_dir(&dir),
        Ok(DirContents::Stored)
    ));
}

// The config is only built when there is something to migrate.
#[test]
fn no_config_is_built_without_a_draft_to_migrate() {
    let (_tmp, fs, _workspace, user) = setup();
    write_dir(&user, "17941219991", &[]);
    let unstored = fs.sanitize().unwrap().unstored;

    store_draft_only_conversations(&fs, &unstored, || {
        panic!("no config is needed for an empty directory")
    });
}
