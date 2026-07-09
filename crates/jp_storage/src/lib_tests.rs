use std::{
    fs::{self, File},
    str::FromStr as _,
};

use camino_tempfile::tempdir;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use test_log::test;

use super::*;
use crate::backend::{ConversationFilter, Projection, StoragePresence};

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
fn load_conversation_index_still_skips_trash() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let convs = tmp.path().join(CONVERSATIONS_DIR);

    fs::create_dir_all(convs.join(".trash")).unwrap();

    let ids: Vec<_> = storage
        .load_conversation_index(ConversationFilter::default())
        .into_iter()
        .map(|entry| entry.id)
        .collect();
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
        .persist_conversation(&id, &metadata, &events, Projection::Projected)
        .unwrap();

    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    assert!(conv_dir.join(METADATA_FILE).is_file());
    assert!(conv_dir.join(BASE_CONFIG_FILE).is_file());
    assert!(conv_dir.join(EVENTS_FILE).is_file());
}

#[test]
fn valid_base_config_edit_survives_load_and_persist() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    // Create the conversation.
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let base_config_path = conv_dir.join(BASE_CONFIG_FILE);

    // A valid manual edit to a recognized field, made outside jp.
    let mut value: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&base_config_path).unwrap()).unwrap();
    value["conversation"]["start_local"] = serde_json::json!(true);
    fs::write(
        &base_config_path,
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();

    // Loading picks up the edited base_config (its mtime is newest); persisting
    // writes the resolved in-memory value back. The valid edit survives; only
    // formatting and unparseable fields are not preserved, which is acceptable.
    let stream = storage.load_conversation_stream(&id).unwrap();
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &stream,
            Projection::Projected,
        )
        .unwrap();

    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&base_config_path).unwrap()).unwrap();
    assert_eq!(
        after["conversation"]["start_local"],
        serde_json::json!(true),
        "a valid manual base_config edit survives load + persist"
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
        .persist_conversation(&id, &metadata, &events, Projection::Projected)
        .unwrap();

    // Add a non-managed file (like QUERY_MESSAGE.md from the editor).
    let conv_dir = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let extra_file = conv_dir.join("QUERY_MESSAGE.md");
    fs::write(&extra_file, "user query content").unwrap();

    // Second persist should not destroy the extra file.
    storage
        .persist_conversation(&id, &metadata, &events, Projection::Projected)
        .unwrap();

    assert!(extra_file.is_file());
    assert_eq!(
        fs::read_to_string(&extra_file).unwrap(),
        "user query content"
    );
}

/// Read a file's mtime as whole seconds since the Unix epoch.
fn mtime_secs(path: &Utf8Path) -> u64 {
    fs::metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[test]
fn projected_persist_converges_base_config_across_roots() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();
    write_valid_stream(&ws_conv);
    write_valid_stream(&user_conv);

    // The two roots' base configs have diverged on disk.
    fs::write(
        ws_conv.join(BASE_CONFIG_FILE),
        r#"{"diverged":"workspace"}"#,
    )
    .unwrap();
    fs::write(user_conv.join(BASE_CONFIG_FILE), r#"{"diverged":"user"}"#).unwrap();

    // A projected persist writes the resolved in-memory base config to both
    // roots, so neither keeps its stale copy.
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    let ws_bc = fs::read_to_string(ws_conv.join(BASE_CONFIG_FILE)).unwrap();
    let user_bc = fs::read_to_string(user_conv.join(BASE_CONFIG_FILE)).unwrap();
    assert_eq!(ws_bc, user_bc, "both roots converge to one base config");
    assert!(
        !ws_bc.contains("diverged"),
        "the stale on-disk base configs are replaced by the resolved one"
    );
}

#[test]
fn unchanged_managed_files_keep_their_mtime() {
    let tmp = tempdir().unwrap();
    let (storage, _workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let meta = Conversation::default();
    let events = ConversationStream::new_test();

    storage
        .persist_conversation(&id, &meta, &events, Projection::LocalOnly)
        .unwrap();

    let conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    for file in [METADATA_FILE, BASE_CONFIG_FILE, EVENTS_FILE] {
        set_mtime(&conv.join(file), 1_000);
    }

    // Re-persisting identical content must not rewrite the files.
    storage
        .persist_conversation(&id, &meta, &events, Projection::LocalOnly)
        .unwrap();

    for file in [METADATA_FILE, BASE_CONFIG_FILE, EVENTS_FILE] {
        assert_eq!(
            mtime_secs(&conv.join(file)),
            1_000,
            "{file} mtime is preserved when content is unchanged"
        );
    }
}

#[cfg(unix)]
#[test]
fn in_place_persist_keeps_conversation_dir_inode() {
    use std::os::unix::fs::MetadataExt as _;

    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let meta = Conversation::default();

    storage
        .persist_conversation(
            &id,
            &meta,
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();
    let conv = tmp.path().join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let inode = fs::metadata(&conv).unwrap().ino();

    // A second persist edits the directory in place rather than swapping it, so
    // the inode (and any shell `cd`'d into it) survives.
    storage
        .persist_conversation(
            &id,
            &meta,
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();
    assert_eq!(
        fs::metadata(&conv).unwrap().ino(),
        inode,
        "in-place persist preserves the conversation directory inode"
    );
}

#[test]
fn title_change_renames_in_place_preserving_extra_files() {
    let tmp = tempdir().unwrap();
    let storage = Storage::new(tmp.path()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let convs = tmp.path().join(CONVERSATIONS_DIR);

    storage
        .persist_conversation(
            &id,
            &Conversation::new("first"),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();
    let first_dir = convs.join(id.to_dirname(Some("first")));
    fs::write(first_dir.join("NOTE.md"), "external note").unwrap();

    // Renaming the conversation moves the directory in place, carrying the
    // non-managed file along, and removes the old-title directory.
    storage
        .persist_conversation(
            &id,
            &Conversation::new("second"),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    let second_dir = convs.join(id.to_dirname(Some("second")));
    assert!(
        second_dir.is_dir(),
        "conversation lives under the new title"
    );
    assert!(!first_dir.exists(), "old-title directory is gone");
    assert_eq!(
        fs::read_to_string(second_dir.join("NOTE.md")).unwrap(),
        "external note",
        "non-managed files survive a title rename"
    );
}

/// Write a minimal conversation directory holding only an `events.json` marker.
///
/// The migration logic keys off directory names and file mtimes, so a real
/// conversation payload is unnecessary here.
fn write_conv_dir(conversations: &Utf8Path, id: &ConversationId, marker: &str) -> Utf8PathBuf {
    let dir = conversations.join(id.to_dirname(None));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(EVENTS_FILE), marker).unwrap();
    dir
}

/// Pin a file's mtime to a fixed instant so conflict resolution is
/// deterministic.
fn set_mtime(path: &Utf8Path, secs: u64) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn with_user_storage_uses_workspace_id_path() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    let storage = Storage::new(workspace.clone())
        .unwrap()
        .with_user_storage(&user_root, None, "abc")
        .unwrap();

    let user_dir = user_root.join("abc");
    assert!(user_dir.is_dir(), "user storage lives at <user_root>/<id>");
    assert_eq!(storage.user_storage_path(), Some(user_dir.as_path()));

    assert!(
        !user_dir.join("storage").is_symlink(),
        "no legacy `storage` symlink is created"
    );
}

#[test]
fn reuses_existing_slugged_user_dir_in_place() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    // An existing `<slug>-<id>` user dir with a conversation, a session
    // mapping, and a stale `storage` symlink pointing at an unrelated path.
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let existing = user_root.join("main-abc");
    write_conv_dir(&existing.join(CONVERSATIONS_DIR), &id, "events");
    fs::create_dir_all(existing.join("sessions")).unwrap();
    fs::write(existing.join("sessions").join("s.json"), "{}").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(tmp.path().join("old-workspace"), existing.join("storage")).unwrap();

    // No slug given: the directory is still located by ID suffix and reused as-is,
    // never renamed to a bare `<id>` directory.
    let storage = Storage::new(workspace.clone())
        .unwrap()
        .with_user_storage(&user_root, None, "abc")
        .unwrap();

    assert_eq!(
        storage.user_storage_path(),
        Some(existing.as_path()),
        "the existing slugged directory is reused in place"
    );
    assert!(
        !user_root.join("abc").exists(),
        "no bare-id directory is created"
    );
    assert!(
        existing
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir(),
        "conversation kept"
    );
    assert!(
        existing.join("sessions").join("s.json").is_file(),
        "session mapping kept"
    );

    #[cfg(unix)]
    {
        let link = existing.join("storage");
        assert!(link.is_symlink());
        assert_eq!(
            fs::read_link(&link).unwrap().as_path(),
            tmp.path().join("old-workspace").as_std_path(),
            "the legacy symlink is left untouched"
        );
    }
}

#[test]
fn merges_sibling_user_dirs_keeping_newest_conversation() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    // The same conversation lives in two user-workspace directories; the `feature-abc` copy is newer
    // and must win the merge.
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let old = write_conv_dir(
        &user_root.join("main-abc").join(CONVERSATIONS_DIR),
        &id,
        "old",
    );
    set_mtime(&old.join(EVENTS_FILE), 1_000);
    let new = write_conv_dir(
        &user_root.join("feature-abc").join(CONVERSATIONS_DIR),
        &id,
        "new",
    );
    set_mtime(&new.join(EVENTS_FILE), 2_000);

    // The slug selects `feature-abc` as the surviving directory; `main-abc` is
    // folded in and removed.
    let storage = Storage::new(workspace)
        .unwrap()
        .with_user_storage(&user_root, Some("feature"), "abc")
        .unwrap();

    let survivor = user_root.join("feature-abc");
    assert_eq!(storage.user_storage_path(), Some(survivor.as_path()));
    assert!(
        !user_root.join("main-abc").exists(),
        "merged sibling removed"
    );

    let merged = survivor.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    assert_eq!(
        fs::read_to_string(merged.join(EVENTS_FILE)).unwrap(),
        "new",
        "the most recently modified copy wins"
    );
}

#[test]
fn creates_slug_prefixed_dir_for_new_workspace() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    let storage = Storage::new(workspace)
        .unwrap()
        .with_user_storage(&user_root, Some("my-project"), "abc")
        .unwrap();

    let dir = user_root.join("my-project-abc");
    assert!(
        dir.is_dir(),
        "a new user-workspace directory is named <slug>-<id>"
    );
    assert_eq!(storage.user_storage_path(), Some(dir.as_path()));
}

#[test]
fn second_clone_reuses_user_workspace_dir_despite_different_slug() {
    let tmp = tempdir().unwrap();
    let user_root = tmp.path().join("user");

    let first = Storage::new(tmp.path().join("clone-a"))
        .unwrap()
        .with_user_storage(&user_root, Some("clone-a"), "abc")
        .unwrap();
    let dir = user_root.join("clone-a-abc");
    assert_eq!(first.user_storage_path(), Some(dir.as_path()));

    // A second clone of the same workspace, in a directory with a different
    // name, reuses the directory created by the first clone rather than minting a
    // `clone-b-abc` of its own.
    let second = Storage::new(tmp.path().join("clone-b"))
        .unwrap()
        .with_user_storage(&user_root, Some("clone-b"), "abc")
        .unwrap();
    assert_eq!(
        second.user_storage_path(),
        Some(dir.as_path()),
        "the existing directory is reused, not renamed"
    );
    assert!(!user_root.join("clone-b-abc").exists());
}

#[test]
fn picks_most_recently_modified_dir_without_slug_match() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    let main = user_root.join("main-abc");
    fs::create_dir_all(&main).unwrap();
    fs::write(main.join("marker"), "a").unwrap();
    set_mtime(&main.join("marker"), 1_000);

    let feature = user_root.join("feature-abc");
    fs::create_dir_all(&feature).unwrap();
    fs::write(feature.join("marker"), "b").unwrap();
    set_mtime(&feature.join("marker"), 2_000);

    // The slug matches neither directory, so the most recently modified one wins.
    let storage = Storage::new(workspace)
        .unwrap()
        .with_user_storage(&user_root, Some("unrelated"), "abc")
        .unwrap();

    assert_eq!(storage.user_storage_path(), Some(feature.as_path()));
    assert!(!main.exists(), "the older sibling is merged in and removed");
}

#[test]
fn imports_workspace_conversations_into_user_local() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = write_conv_dir(&workspace.join(CONVERSATIONS_DIR), &id, "ws events");

    Storage::new(workspace)
        .unwrap()
        .with_user_storage(&user_root, None, "abc")
        .unwrap();

    assert!(
        user_root
            .join("abc")
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir(),
        "workspace conversation imported into user-local"
    );
    assert!(ws_conv.exists(), "workspace copy is preserved");
}

#[test]
fn imports_archived_workspace_conversations() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let archive = workspace.join(CONVERSATIONS_DIR).join(ARCHIVE_DIR);
    let ws_conv = write_conv_dir(&archive, &id, "archived events");

    Storage::new(workspace)
        .unwrap()
        .with_user_storage(&user_root, None, "abc")
        .unwrap();

    assert!(
        user_root
            .join("abc")
            .join(CONVERSATIONS_DIR)
            .join(ARCHIVE_DIR)
            .join(id.to_dirname(None))
            .is_dir(),
        "archived workspace conversation imported into user-local"
    );
    assert!(ws_conv.exists(), "workspace archive copy is preserved");
}

#[test]
fn import_runs_only_on_first_establishment() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let user_root = tmp.path().join("user");

    // First run establishes the user-local store (no conversations yet).
    Storage::new(workspace.clone())
        .unwrap()
        .with_user_storage(&user_root, None, "abc")
        .unwrap();
    assert!(user_root.join("abc").is_dir());

    // A conversation committed by another contributor appears later.
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    write_conv_dir(&workspace.join(CONVERSATIONS_DIR), &id, "external");

    // A later run must not eagerly import it; that is left to lazy import on
    // first write.
    Storage::new(workspace)
        .unwrap()
        .with_user_storage(&user_root, None, "abc")
        .unwrap();

    assert!(
        !user_root
            .join("abc")
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .exists(),
        "external conversations are not imported after the one-time migration"
    );
}

/// Write a valid `metadata.json` carrying `title`, with the given mtime.
fn write_meta(conv_dir: &Utf8Path, title: Option<&str>, mtime_secs: u64) {
    let conv = Conversation {
        title: title.map(str::to_owned),
        ..Default::default()
    };
    fs::write(
        conv_dir.join(METADATA_FILE),
        serde_json::to_string(&conv).unwrap(),
    )
    .unwrap();
    set_mtime(&conv_dir.join(METADATA_FILE), mtime_secs);
}

/// Write an `events.json` array with `count` entries and the given mtime.
///
/// The entries are shaped for the count/last-timestamp reader, not for full
/// stream parsing.
fn write_events(conv_dir: &Utf8Path, count: usize, mtime_secs: u64) {
    let events: Vec<_> = (1..=count)
        .map(|day| serde_json::json!({ "timestamp": format!("2024-01-{day:02}T00:00:00Z") }))
        .collect();
    fs::write(
        conv_dir.join(EVENTS_FILE),
        serde_json::to_string(&events).unwrap(),
    )
    .unwrap();
    set_mtime(&conv_dir.join(EVENTS_FILE), mtime_secs);
}

/// Write a placeholder `base_config.json` with the given mtime.
fn write_base_config(conv_dir: &Utf8Path, mtime_secs: u64) {
    fs::write(conv_dir.join(BASE_CONFIG_FILE), "{}").unwrap();
    set_mtime(&conv_dir.join(BASE_CONFIG_FILE), mtime_secs);
}

/// Write a valid, loadable conversation stream (`base_config.json` +
/// `events.json`).
fn write_valid_stream(conv_dir: &Utf8Path) {
    let (base_config, events) = ConversationStream::new_test().to_parts().unwrap();
    fs::write(
        conv_dir.join(BASE_CONFIG_FILE),
        serde_json::to_string(&base_config).unwrap(),
    )
    .unwrap();
    fs::write(
        conv_dir.join(EVENTS_FILE),
        serde_json::to_string(&events).unwrap(),
    )
    .unwrap();
}

/// A `Storage` with both a workspace and a user-local root, plus the two paths.
fn dual_root_storage(tmp: &Utf8Path) -> (Storage, Utf8PathBuf, Utf8PathBuf) {
    let workspace = tmp.join("workspace");
    let user_root = tmp.join("user");
    let storage = Storage::new(workspace.clone())
        .unwrap()
        .with_user_storage(&user_root, None, "ws")
        .unwrap();
    (storage, workspace, user_root.join("ws"))
}

#[test]
fn index_dedups_and_classifies_presence() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let ws_convs = workspace.join(CONVERSATIONS_DIR);
    let user_convs = user_dir.join(CONVERSATIONS_DIR);

    let only_workspace = ConversationId::try_from_deciseconds_str("17636257521").unwrap();
    let only_user = ConversationId::try_from_deciseconds_str("17636257522").unwrap();
    let projected = ConversationId::try_from_deciseconds_str("17636257523").unwrap();

    write_conv_dir(&ws_convs, &only_workspace, "x");
    write_conv_dir(&user_convs, &only_user, "x");
    write_conv_dir(&ws_convs, &projected, "x");
    write_conv_dir(&user_convs, &projected, "x");

    let entries = storage.load_conversation_index(ConversationFilter::default());
    assert_eq!(
        entries.len(),
        3,
        "the projected conversation is not duplicated"
    );

    let presence = |id: ConversationId| {
        entries
            .iter()
            .find(|entry| entry.id == id)
            .unwrap()
            .presence
    };
    assert_eq!(presence(only_workspace), StoragePresence::WorkspaceOnly);
    assert_eq!(presence(only_user), StoragePresence::UserLocalOnly);
    assert_eq!(presence(projected), StoragePresence::Projected);
}

#[test]
fn metadata_resolves_by_newer_mtime() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();

    write_meta(&ws_conv, Some("workspace title"), 1_000);
    write_events(&ws_conv, 1, 1_000);
    write_meta(&user_conv, Some("user title"), 2_000);
    write_events(&user_conv, 1, 1_000);

    let conv = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conv.title.as_deref(),
        Some("user title"),
        "newer metadata wins"
    );

    let presence = storage
        .load_conversation_index(ConversationFilter::default())
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap()
        .presence;
    assert_eq!(
        presence,
        StoragePresence::Projected,
        "a conversation present in both roots is projected"
    );
}

#[test]
fn metadata_tie_prefers_user_local() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();

    write_meta(&ws_conv, Some("workspace title"), 1_000);
    write_events(&ws_conv, 1, 1_000);
    write_meta(&user_conv, Some("user title"), 1_000);
    write_events(&user_conv, 1, 1_000);

    let conv = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conv.title.as_deref(),
        Some("user title"),
        "equal mtimes resolve to the durable user-local copy"
    );
}

#[test]
fn events_count_comes_from_newer_stream() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();

    write_meta(&ws_conv, None, 1_000);
    write_meta(&user_conv, None, 1_000);
    write_events(&ws_conv, 1, 1_000);
    write_events(&user_conv, 2, 2_000);

    let conv = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conv.events_count, 2,
        "event count is read from the newer stream root"
    );
}

#[test]
fn base_config_mtime_counts_toward_stream_freshness() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();

    write_meta(&ws_conv, None, 1_000);
    write_meta(&user_conv, None, 1_000);

    // The user events are newer, but the workspace base_config is newer still,
    // so the workspace stream wins as a unit.
    write_events(&ws_conv, 1, 1_000);
    write_base_config(&ws_conv, 5_000);
    write_events(&user_conv, 2, 2_000);
    write_base_config(&user_conv, 500);

    let conv = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conv.events_count, 1,
        "base_config mtime makes the workspace stream the newer unit"
    );
}

#[test]
fn stream_loads_from_newer_root() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());

    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();

    // Newer, valid stream in user-local; older, corrupt copy in the workspace.
    write_valid_stream(&user_conv);
    set_mtime(&user_conv.join(EVENTS_FILE), 2_000);
    set_mtime(&user_conv.join(BASE_CONFIG_FILE), 2_000);
    fs::write(ws_conv.join(EVENTS_FILE), "not valid json").unwrap();
    set_mtime(&ws_conv.join(EVENTS_FILE), 1_000);

    let stream = storage.load_conversation_stream(&id);
    assert!(
        stream.is_ok(),
        "the newer user-local stream is selected over the stale workspace copy"
    );
}

#[test]
fn projection_and_presence_map_to_each_other() {
    assert_eq!(
        Projection::from(StoragePresence::UserLocalOnly),
        Projection::LocalOnly
    );
    assert_eq!(
        Projection::from(StoragePresence::Projected),
        Projection::Projected
    );
    assert_eq!(
        Projection::from(StoragePresence::WorkspaceOnly),
        Projection::Projected
    );
    assert_eq!(
        StoragePresence::from(Projection::LocalOnly),
        StoragePresence::UserLocalOnly
    );
    assert_eq!(
        StoragePresence::from(Projection::Projected),
        StoragePresence::Projected
    );
}

#[test]
fn projected_write_reaches_both_roots() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    assert!(
        workspace
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir(),
        "projected conversation is written to the workspace"
    );
    assert!(
        user_dir
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir(),
        "projected conversation is written to user-local"
    );
}

#[test]
fn local_only_write_skips_workspace() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::LocalOnly,
        )
        .unwrap();

    assert!(
        user_dir
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir(),
        "local-only conversation is written to user-local"
    );
    assert!(
        !workspace
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .exists(),
        "local-only conversation has no workspace projection"
    );
}

#[test]
fn dual_write_does_not_clobber_the_other_root() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    // Two projected persists: per-root stale cleanup must not delete the copy
    // just written in the other root.
    for _ in 0..2 {
        storage
            .persist_conversation(
                &id,
                &Conversation::default(),
                &ConversationStream::new_test(),
                Projection::Projected,
            )
            .unwrap();
    }

    assert!(
        workspace
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir()
    );
    assert!(
        user_dir
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir()
    );
}

#[test]
fn write_without_user_storage_ignores_projection() {
    let tmp = tempdir().unwrap();
    let workspace = tmp.path().join("workspace");
    let storage = Storage::new(workspace.clone()).unwrap();
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    // `LocalOnly` has no user-local root to target, so it falls back to a
    // single workspace write.
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::LocalOnly,
        )
        .unwrap();

    assert!(
        workspace
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir()
    );
}

#[test]
fn archive_and_unarchive_act_on_every_root() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let active = |root: &Utf8Path| root.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let archived = |root: &Utf8Path| {
        root.join(CONVERSATIONS_DIR)
            .join(ARCHIVE_DIR)
            .join(id.to_dirname(None))
    };

    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    storage.archive_conversation(&id).unwrap();
    assert!(!active(&workspace).exists() && !active(&user_dir).exists());
    assert!(archived(&workspace).is_dir() && archived(&user_dir).is_dir());

    storage.unarchive_conversation(&id).unwrap();
    assert!(active(&workspace).is_dir() && active(&user_dir).is_dir());
    assert!(!archived(&workspace).exists() && !archived(&user_dir).exists());
}

#[test]
fn projected_write_imports_external_non_managed_files() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    // A workspace-only conversation, as if committed by another contributor,
    // carrying a non-managed file alongside its managed files.
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    write_valid_stream(&ws_conv);
    fs::write(
        ws_conv.join(METADATA_FILE),
        serde_json::to_string(&Conversation::default()).unwrap(),
    )
    .unwrap();
    fs::write(ws_conv.join("NOTE.md"), "external note").unwrap();

    // The first durable write imports the workspace copy, then dual-writes.
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    assert!(user_conv.is_dir(), "conversation imported into user-local");
    assert_eq!(
        fs::read_to_string(user_conv.join("NOTE.md")).unwrap(),
        "external note",
        "a non-managed file survives the import"
    );
}

#[test]
fn projection_toggle_adds_and_removes_workspace_copy() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));

    // Projected: both roots hold the conversation.
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();
    assert!(ws_conv.is_dir() && user_conv.is_dir());

    // Toggle to local-only: the workspace projection is dropped, the durable
    // user-local copy stays.
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::LocalOnly,
        )
        .unwrap();
    assert!(user_conv.is_dir(), "user-local copy remains");
    assert!(!ws_conv.exists(), "workspace projection removed");

    // Toggle back to projected: the workspace projection is recreated.
    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();
    assert!(
        ws_conv.is_dir() && user_conv.is_dir(),
        "workspace projection restored"
    );
}

#[test]
fn find_conversation_dir_prefers_workspace_for_projected() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, _user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    let dir = storage.find_conversation_dir(&id).unwrap();
    assert!(
        dir.starts_with(&workspace),
        "a projected conversation resolves to the workspace path: {dir}"
    );
}

#[test]
fn sync_projection_overwrites_user_local_from_workspace() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    // Simulate a managed editor command editing the workspace copy.
    let ws_events = workspace
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None))
        .join(EVENTS_FILE);
    fs::write(&ws_events, "[]").unwrap();

    storage.sync_projection(&id).unwrap();

    let user_events = user_dir
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None))
        .join(EVENTS_FILE);
    assert_eq!(
        fs::read_to_string(&user_events).unwrap(),
        "[]",
        "the user-local copy is synced from the edited workspace copy"
    );
}

#[test]
fn sync_projection_is_noop_for_local_only() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::LocalOnly,
        )
        .unwrap();

    storage.sync_projection(&id).unwrap();

    assert!(
        user_dir
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .is_dir(),
        "the user-local copy is untouched"
    );
    assert!(
        !workspace
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(None))
            .exists(),
        "no workspace copy is created for a local-only conversation"
    );
}

#[test]
fn metadata_and_stream_resolve_independently() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();

    // Workspace has the newer metadata but the older (1-event) stream;
    // user-local has the older metadata but the newer (2-event) stream.
    write_meta(&ws_conv, Some("workspace title"), 2_000);
    write_events(&ws_conv, 1, 1_000);
    write_meta(&user_conv, Some("user title"), 1_000);
    write_events(&user_conv, 2, 2_000);

    let conv = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conv.title.as_deref(),
        Some("workspace title"),
        "metadata comes from the newer-metadata root"
    );
    assert_eq!(
        conv.events_count, 2,
        "event count comes from the newer-stream root, resolved independently"
    );
}

#[test]
fn stream_resolution_ties_to_user_local() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
    let ws_conv = workspace.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    let user_conv = user_dir.join(CONVERSATIONS_DIR).join(id.to_dirname(None));
    fs::create_dir_all(&ws_conv).unwrap();
    fs::create_dir_all(&user_conv).unwrap();

    write_meta(&ws_conv, None, 1_000);
    write_meta(&user_conv, None, 1_000);
    // Equal stream mtimes: the durable user-local copy wins the tie.
    write_events(&ws_conv, 1, 1_000);
    write_events(&user_conv, 2, 1_000);

    let conv = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(
        conv.events_count, 2,
        "an equal stream mtime resolves to the user-local copy"
    );
}

#[test]
fn dual_write_renames_in_both_roots_without_clobbering() {
    let tmp = tempdir().unwrap();
    let (storage, workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    let persist = |title: &str| {
        storage
            .persist_conversation(
                &id,
                &Conversation {
                    title: Some(title.to_owned()),
                    ..Default::default()
                },
                &ConversationStream::new_test(),
                Projection::Projected,
            )
            .unwrap();
    };

    persist("first");
    persist("second");

    // The per-root stale cleanup renames the copy in each root and never
    // deletes the copy just written in the other root.
    for convs in [
        workspace.join(CONVERSATIONS_DIR),
        user_dir.join(CONVERSATIONS_DIR),
    ] {
        assert!(
            convs.join(id.to_dirname(Some("second"))).is_dir(),
            "renamed copy present in {convs}"
        );
        assert!(
            !convs.join(id.to_dirname(Some("first"))).exists(),
            "stale-title copy removed in {convs}"
        );
    }
}

#[test]
fn find_user_local_conversation_dir_tracks_rename() {
    let tmp = tempdir().unwrap();
    let (storage, _workspace, user_dir) = dual_root_storage(tmp.path());
    let id = ConversationId::try_from_deciseconds_str("17636257526").unwrap();

    storage
        .persist_conversation(
            &id,
            &Conversation::default(),
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    // A title set on a later persist (e.g. a heading-derived title) renames the
    // directory in every root, leaving any path captured against the untitled
    // name stale. The resolver re-derives the live user-local name.
    storage
        .persist_conversation(
            &id,
            &Conversation {
                title: Some("heading title".to_owned()),
                ..Default::default()
            },
            &ConversationStream::new_test(),
            Projection::Projected,
        )
        .unwrap();

    assert_eq!(
        storage.find_user_local_conversation_dir(&id).unwrap(),
        user_dir
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(Some("heading title"))),
        "resolves to the renamed user-local copy"
    );
}
