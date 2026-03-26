use std::{collections::HashMap, fs, time::Duration};

use camino_tempfile::tempdir;
use datetime_literal::datetime;
use jp_config::{
    PartialConfig as _,
    fs::load_partial,
    model::id::{ModelIdOrAliasConfig, PartialModelIdOrAliasConfig, ProviderId},
    util::build,
};
use jp_conversation::ConversationsMetadata;
use jp_storage::{
    CONVERSATIONS_DIR, METADATA_FILE,
    value::{read_json, write_json},
};
use test_log::test;

use super::*;

#[test]
fn test_workspace_find_root() {
    struct TestCase {
        workspace_dir: &'static str,
        workspace_dir_name: Option<&'static str>,
        workspace_dir_name_is_file: bool,
        cwd: &'static str,
        expected: Option<&'static str>,
    }

    let workspace_dir_name = Some("test_workspace");
    let workspace_dir_name_is_file = false;

    let test_cases = HashMap::from([
        ("workspace in current directory", TestCase {
            workspace_dir: "project",
            workspace_dir_name,
            workspace_dir_name_is_file,
            cwd: "project",
            expected: Some("project"),
        }),
        ("workspace in parent directory", TestCase {
            workspace_dir: "project",
            workspace_dir_name,
            workspace_dir_name_is_file,
            cwd: "project/subdir",
            expected: Some("project"),
        }),
        ("workspace in grandparent directory", TestCase {
            workspace_dir: "project",
            workspace_dir_name,
            workspace_dir_name_is_file,
            cwd: "project/subdir/subsubdir",
            expected: Some("project"),
        }),
        ("no workspace directory", TestCase {
            workspace_dir: "project",
            workspace_dir_name: None,
            workspace_dir_name_is_file,
            cwd: "project",
            expected: None,
        }),
        ("workspace name is a file", TestCase {
            workspace_dir: "project",
            workspace_dir_name,
            workspace_dir_name_is_file: true,
            cwd: "project",
            expected: None,
        }),
        ("different workspace name", TestCase {
            workspace_dir: "project",
            workspace_dir_name: Some("different_name"),
            workspace_dir_name_is_file,
            cwd: "project",
            expected: None,
        }),
        ("empty workspace name", TestCase {
            workspace_dir: "project",
            workspace_dir_name: Some(""),
            workspace_dir_name_is_file,
            cwd: "project",
            expected: None,
        }),
    ]);

    for (name, case) in test_cases {
        #[allow(clippy::unnecessary_literal_unwrap)]
        let workspace_dir_name = workspace_dir_name.unwrap();

        let root = tempdir().unwrap().path().to_path_buf();
        let cwd = root.join(case.cwd);
        let project = root.join(case.workspace_dir);
        let expected = case.expected.map(|v| root.join(v));

        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&project).unwrap();

        if case.workspace_dir_name.is_some() {
            if case.workspace_dir_name_is_file {
                fs::write(project.join(workspace_dir_name), "").unwrap();
            } else {
                fs::create_dir_all(project.join(workspace_dir_name)).unwrap();
            }
        }

        let result = Workspace::find_root(cwd, case.workspace_dir_name.unwrap_or("non-exist"));
        assert_eq!(result, expected, "Failed test case: {name}");
    }
}

#[test]
fn test_workspace_persist_saves_in_memory_state() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    let mut workspace = Workspace::new(&root);
    let config = AppConfig::new_test();

    let id = workspace.create_conversation(Conversation::default(), config.into());
    workspace
        .set_active_conversation_id(id, DateTime::<Utc>::UNIX_EPOCH)
        .unwrap();
    assert!(!storage.exists());

    // Persisting without a storage should be a no-op.
    workspace.persist().unwrap();

    let mut workspace = workspace.persisted_at(&storage).unwrap();
    workspace.persist().unwrap();
    assert!(storage.is_dir());

    let conversation_id = workspace.conversations().next().unwrap().0;
    let metadata_file = storage
        .join(CONVERSATIONS_DIR)
        .join(conversation_id.to_dirname(None))
        .join(METADATA_FILE);

    assert!(metadata_file.is_file());

    let _metadata: Conversation = read_json(&metadata_file).unwrap();
}

#[test]
fn test_workspace_conversations() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert_eq!(workspace.conversations().count(), 1); // Default conversation

    let id = ConversationId::default();
    let conversation = Conversation::default();
    workspace
        .state
        .local
        .conversations
        .entry(id)
        .or_default()
        .set(conversation)
        .unwrap();
    assert_eq!(workspace.conversations().count(), 2);
}

#[test]
fn test_workspace_get_conversation() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert!(workspace.state.local.conversations.is_empty());

    let id = ConversationId::try_from(Utc::now() - Duration::from_secs(1)).unwrap();
    assert_eq!(workspace.get_conversation(&id), None);

    let conversation = Conversation::default();
    workspace
        .state
        .local
        .conversations
        .entry(id)
        .or_default()
        .set(conversation.clone())
        .unwrap();
    assert_eq!(workspace.get_conversation(&id), Some(&conversation));
}

#[test]
fn test_workspace_create_conversation() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert!(workspace.state.local.conversations.is_empty());

    let conversation = Conversation::default();
    let config = AppConfig::new_test();
    let id = workspace.create_conversation(conversation.clone(), config.into());

    assert_eq!(
        workspace
            .state
            .local
            .conversations
            .get(&id)
            .and_then(|v| v.get()),
        Some(&conversation)
    );
}

#[test]
fn test_workspace_remove_conversation() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert!(workspace.state.local.conversations.is_empty());

    let id = ConversationId::try_from(Utc::now() - Duration::from_secs(1)).unwrap();
    let conversation = Conversation::default();
    workspace
        .state
        .local
        .conversations
        .entry(id)
        .or_default()
        .set(conversation.clone())
        .unwrap();

    assert_ne!(workspace.active_conversation_id(), id);
    let removed_conversation = workspace.remove_conversation(&id).unwrap().unwrap();
    assert_eq!(removed_conversation, conversation);
    assert!(workspace.state.local.conversations.is_empty());
}

#[test]
fn test_workspace_cannot_remove_active_conversation() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert!(workspace.state.local.conversations.is_empty());

    let active_id = workspace
        .state
        .user
        .conversations_metadata
        .active_conversation_id;
    let active_conversation = workspace.state.local.active_conversation.clone();

    assert!(workspace.remove_conversation(&active_id).is_err());
    assert_eq!(
        workspace.state.local.active_conversation,
        active_conversation
    );
}

#[test]
fn test_load_index_fresh_workspace_then_ensure_stream() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    let missing_id = ConversationId::try_from(datetime!(2024-06-01 00:00:00 Z)).unwrap();

    fs::create_dir_all(&storage).unwrap();
    write_conversations_metadata_to_disk(&storage, &missing_id);

    let mut workspace = Workspace::new(&root).persisted_at(&storage).unwrap();
    workspace.disable_persistence();

    // Phase 1: load index — no conversations on disk, so the active
    // conversation entry is registered but has no stream yet.
    workspace.load_conversation_index().unwrap();
    let active_id = workspace.active_conversation_id();
    assert!(
        workspace.get_events(&active_id).is_none(),
        "fresh workspace should have no stream before ensure_active_conversation_stream"
    );

    // Phase 2: create the default stream with the final config.
    let config = Arc::new(AppConfig::new_test());
    workspace.ensure_active_conversation_stream(config).unwrap();
    assert!(workspace.get_events(&active_id).is_some());
}

#[test]
fn test_load_index_existing_workspace_events_accessible() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage_path = root.join("storage");

    let config = Arc::new(AppConfig::new_test());
    let id = ConversationId::try_from(datetime!(2024-03-15 12:00:00 Z)).unwrap();

    // Write a conversation to disk.
    {
        let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
        ws.create_conversation_with_id(id, Conversation::default(), config.clone());
        ws.set_active_conversation_id(id, DateTime::<Utc>::UNIX_EPOCH)
            .unwrap();
        ws.persist().unwrap();
    }

    // Reload from scratch — only load_conversation_index, no config needed.
    let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
    ws.disable_persistence();
    ws.load_conversation_index().unwrap();

    // Events should be accessible via lazy loading.
    assert_eq!(ws.active_conversation_id(), id);
    let events = ws.get_events(&id);
    assert!(
        events.is_some(),
        "events must be lazily loadable after load_conversation_index"
    );

    // The stream's config should be retrievable (this is what
    // apply_conversation_config relies on).
    let stream_config = events.unwrap().config();
    assert!(stream_config.is_ok());

    // ensure_active_conversation_stream should be a no-op.
    ws.ensure_active_conversation_stream(config).unwrap();
    assert!(ws.get_events(&id).is_some());
}

/// Regression test for the bug where continuing a conversation without the
/// original config passed in via `--cfg` caused a spurious `ConfigDelta` that
/// reset the model and disabled all tools.
///
/// The root cause was that `load_partial_config` ran before conversation events
/// were loaded from disk, so `apply_conversation_config` couldn't read the
/// stream config and fell back to an empty partial.
///
/// This test exercises the full round-trip:
/// 1. Create a conversation with a custom model name, persist to disk.
/// 2. Reload with `load_conversation_index` only (no config needed).
/// 3. Simulate `apply_conversation_config`: merge stream config into a bare
///    partial (as if no custom config was passed).
/// 4. Build the final config and assert the custom model name survived.
/// 5. Assert that `get_config_delta_from_cli` would produce no delta.
#[test]
fn test_conversation_config_preserved_across_reload() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage_path = root.join("storage");

    // Build a config with a distinctive model name.
    let mut custom_config = AppConfig::new_test();
    custom_config.assistant.model.id =
        ModelIdOrAliasConfig::Id((ProviderId::Anthropic, "custom-model").try_into().unwrap());
    let id = ConversationId::try_from(datetime!(2024-05-20 10:00:00 Z)).unwrap();

    // Persist a conversation that was created with the custom config.
    {
        let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
        ws.create_conversation_with_id(id, Conversation::default(), Arc::new(custom_config));
        ws.set_active_conversation_id(id, DateTime::<Utc>::UNIX_EPOCH)
            .unwrap();
        ws.persist().unwrap();
    }

    // Simulate a second invocation WITHOUT the custom config.
    let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
    ws.disable_persistence();
    ws.load_conversation_index().unwrap();

    // Simulate `apply_conversation_config`: merge the stream's config into a
    // bare (default) partial, exactly as the CLI does when no `--cfg` flag is
    // provided. We use new_test().to_partial() to represent the default config
    // (file-based + env, but no custom config overlay).
    let bare_partial = AppConfig::new_test().to_partial();
    let stream_config = ws
        .get_events(&id)
        .expect("events must be accessible")
        .config()
        .expect("valid config")
        .to_partial();
    let merged = load_partial(bare_partial, stream_config).expect("merge ok");
    let final_config = build(merged).expect("valid config");

    // The custom config's model name must survive the round-trip.
    let resolved = final_config.assistant.model.id.resolved();
    assert_eq!(
        resolved.name.as_ref(),
        "custom-model",
        "conversation config must be preserved when continuing without --cfg flag"
    );

    // The delta must NOT contain a model change — this was the core symptom of
    // the bug, where the model reverted to the default.
    let stream_partial = ws.get_events(&id).unwrap().config().unwrap().to_partial();
    let delta = stream_partial.delta(final_config.to_partial());
    assert_eq!(
        delta.assistant.model.id,
        PartialModelIdOrAliasConfig::empty(),
        "config delta must not contain a model change when continuing a conversation without \
         overrides"
    );
}

#[test]
fn test_workspace_persist_active_conversation() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    let mut workspace = Workspace::new(&root).persisted_at(&storage).unwrap();
    let config = Arc::new(AppConfig::new_test());

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();

    workspace.create_conversation_with_id(id1, Conversation::default(), config.clone());
    workspace.create_conversation_with_id(id2, Conversation::default(), config.clone());
    workspace
        .set_active_conversation_id(id1, DateTime::<Utc>::UNIX_EPOCH)
        .unwrap();

    workspace.persist_active_conversation().unwrap();
    assert!(storage.is_dir());

    let id1_metadata_file = storage
        .join(CONVERSATIONS_DIR)
        .join(id1.to_dirname(None))
        .join(METADATA_FILE);

    let id2_metadata_file = storage
        .join(CONVERSATIONS_DIR)
        .join(id2.to_dirname(None))
        .join(METADATA_FILE);

    assert!(id1_metadata_file.is_file());
    assert!(!id2_metadata_file.is_file());
}

/// Regression test: files placed in a local conversation's user-storage
/// directory must survive `persist_active_conversation`.
///
/// Before the fix, the editor created `QUERY_MESSAGE.md` inside the
/// workspace-side `.jp/conversations/{id}/` directory, even for local
/// conversations. `persist_active_conversation` then deleted that
/// workspace-side directory (because the conversation lives in user
/// storage), destroying the query file and causing an IO error.
///
/// The fix ensures local conversations use the user-storage path for
/// the editor file. This test verifies that a file placed in the
/// correct (user-storage) conversation directory is not deleted by
/// `persist_active_conversation`.
#[test]
fn test_persist_active_conversation_preserves_files_in_user_storage() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage_path = root.join("storage");
    let user_root = tmp.path().join("user");

    let config = Arc::new(AppConfig::new_test());
    let id = ConversationId::try_from(datetime!(2024-07-01 00:00:00 Z)).unwrap();

    let mut workspace = Workspace::new(&root)
        .persisted_at(&storage_path)
        .unwrap()
        .with_local_storage_at(&user_root, "test-ws", "abc")
        .unwrap();

    // Create a local conversation and make it active.
    let conversation = Conversation::default().with_local(true);
    workspace.create_conversation_with_id(id, conversation, config);
    workspace
        .set_active_conversation_id(id, DateTime::<Utc>::UNIX_EPOCH)
        .unwrap();

    // Simulate what the editor does: place a file inside the conversation's
    // directory. For local conversations, this should be in user storage.
    let user_storage = workspace.user_storage_path().unwrap();
    let conv_dir = user_storage
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None));
    fs::create_dir_all(&conv_dir).unwrap();
    let query_file = conv_dir.join("QUERY_MESSAGE.md");
    fs::write(&query_file, "test query content").unwrap();
    assert!(query_file.is_file());

    // This is the operation that previously deleted the workspace-side
    // directory. With the file in user storage, it should be preserved.
    workspace.persist_active_conversation().unwrap();

    assert!(
        query_file.is_file(),
        "query file in user-storage conversation directory must survive persist_active_conversation"
    );

    // The workspace-side conversations directory should NOT have a
    // directory for this conversation (it's local-only).
    let workspace_conv_dir = storage_path
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None));
    assert!(
        !workspace_conv_dir.exists(),
        "local conversation should not create a workspace-side directory"
    );
}

/// Write a `conversations/metadata.json` pointing to the given active ID.
fn write_conversations_metadata_to_disk(storage: &Utf8Path, active_id: &ConversationId) {
    let meta_path = storage.join(CONVERSATIONS_DIR).join(METADATA_FILE);
    let meta = ConversationsMetadata::new(*active_id);

    write_json(&meta_path, &meta).unwrap();
}
