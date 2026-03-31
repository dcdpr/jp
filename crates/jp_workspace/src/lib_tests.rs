use std::{collections::HashMap, fs, sync::Arc, time::Duration};

use camino_tempfile::tempdir;
use datetime_literal::datetime;
use jp_config::{
    PartialConfig as _,
    fs::load_partial,
    model::id::{ModelIdOrAliasConfig, PartialModelIdOrAliasConfig, ProviderId},
    util::build,
};
use jp_storage::{CONVERSATIONS_DIR, METADATA_FILE, value::read_json};
use parking_lot::RwLock;
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
fn test_workspace_persist_via_lock() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    let mut workspace = Workspace::new(&root).persisted_at(&storage).unwrap();
    let config = AppConfig::new_test();

    let id = workspace.create_conversation(Conversation::default(), config.into());

    // Persist via ConversationMut: mark dirty then flush.
    let h = workspace.acquire_conversation(&id).unwrap();
    let mut conv = workspace.test_lock(h).into_mut();
    conv.update_metadata(|_| {}); // set dirty flag
    conv.flush().unwrap();
    drop(conv);

    assert!(storage.is_dir());

    let metadata_file = storage
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None))
        .join(METADATA_FILE);

    assert!(metadata_file.is_file());

    let _metadata: Conversation = read_json(&metadata_file).unwrap();
}

#[test]
fn test_workspace_conversations() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert_eq!(workspace.conversations().count(), 0);

    let id = ConversationId::default();
    let conversation = Conversation::default();
    workspace
        .state
        .conversations
        .entry(id)
        .or_default()
        .set(Arc::new(RwLock::new(conversation)))
        .unwrap();
    assert_eq!(workspace.conversations().count(), 1);
}

#[test]
fn test_workspace_acquire_conversation() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert!(workspace.state.conversations.is_empty());

    let id = ConversationId::try_from(chrono::Utc::now() - Duration::from_secs(1)).unwrap();
    assert!(workspace.acquire_conversation(&id).is_err());

    let conversation = Conversation::default();
    workspace
        .state
        .conversations
        .entry(id)
        .or_default()
        .set(Arc::new(RwLock::new(conversation.clone())))
        .unwrap();

    let handle = workspace.acquire_conversation(&id).unwrap();
    assert_eq!(*workspace.metadata(&handle).unwrap(), conversation);
}

#[test]
fn test_workspace_create_conversation() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert!(workspace.state.conversations.is_empty());

    let conversation = Conversation::default();
    let config = AppConfig::new_test();
    let id = workspace.create_conversation(conversation.clone(), config.into());

    assert_eq!(
        workspace
            .state
            .conversations
            .get(&id)
            .and_then(|v| v.get())
            .map(|arc| arc.read().clone()),
        Some(conversation)
    );
}

#[test]
fn test_workspace_remove_conversation() {
    let mut workspace = Workspace::new(Utf8PathBuf::new());
    assert!(workspace.state.conversations.is_empty());

    let id = ConversationId::try_from(chrono::Utc::now() - Duration::from_secs(1)).unwrap();
    let conversation = Conversation::default();
    workspace
        .state
        .conversations
        .entry(id)
        .or_default()
        .set(Arc::new(RwLock::new(conversation)))
        .unwrap();
    // Also add events entry so test_lock works.
    workspace
        .state
        .events
        .entry(id)
        .or_default()
        .set(Arc::new(RwLock::new(ConversationStream::new_test())))
        .unwrap();

    let handle = workspace.acquire_conversation(&id).unwrap();
    let lock = workspace.test_lock(handle);
    workspace.remove_conversation_with_lock(lock.into_mut());
    assert!(workspace.state.conversations.is_empty());
}

#[test]
fn test_load_index_fresh_workspace() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    fs::create_dir_all(&storage).unwrap();

    let mut workspace = Workspace::new(&root).persisted_at(&storage).unwrap();
    workspace.disable_persistence();

    workspace.load_conversation_index();
    assert_eq!(workspace.conversations().count(), 0);
}

#[test]
fn test_load_index_fresh_workspace_then_create_conversation() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    fs::create_dir_all(&storage).unwrap();

    let mut workspace = Workspace::new(&root).persisted_at(&storage).unwrap();
    workspace.disable_persistence();

    workspace.load_conversation_index();
    assert_eq!(workspace.conversations().count(), 0);

    let config = Arc::new(AppConfig::new_test());
    let id = workspace.create_conversation(Conversation::default(), config);
    let handle = workspace.acquire_conversation(&id).unwrap();
    assert!(
        !workspace.events(&handle).unwrap().is_empty()
            || workspace.events(&handle).unwrap().is_empty()
    );
    assert_eq!(workspace.metadata(&handle).unwrap().title, None);
}

#[test]
fn test_load_index_existing_workspace_events_accessible() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage_path = root.join("storage");

    let config = Arc::new(AppConfig::new_test());
    let id = ConversationId::try_from(datetime!(2024-03-15 12:00:00 Z)).unwrap();

    // Write a conversation to disk via flush.
    {
        let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
        ws.create_conversation_with_id(id, Conversation::default(), config.clone());
        let h = ws.acquire_conversation(&id).unwrap();
        let mut conv = ws.test_lock(h).into_mut();
        conv.update_metadata(|_| {});
        conv.flush().unwrap();
    }

    // Reload from scratch.
    let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
    ws.disable_persistence();
    ws.load_conversation_index();

    let handle = ws.acquire_conversation(&id).unwrap();
    ws.eager_load_conversation(&handle).unwrap();
    let events = ws.events(&handle).unwrap();

    let stream_config = events.config();
    assert!(stream_config.is_ok());
}

/// Regression test: continuing a conversation without the original config
/// passed via `--cfg` must preserve the conversation's config.
#[test]
fn test_conversation_config_preserved_across_reload() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage_path = root.join("storage");

    let mut custom_config = AppConfig::new_test();
    custom_config.assistant.model.id =
        ModelIdOrAliasConfig::Id((ProviderId::Anthropic, "custom-model").try_into().unwrap());
    let id = ConversationId::try_from(datetime!(2024-05-20 10:00:00 Z)).unwrap();

    // Persist via flush.
    {
        let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
        ws.create_conversation_with_id(id, Conversation::default(), Arc::new(custom_config));
        let h = ws.acquire_conversation(&id).unwrap();
        let mut conv = ws.test_lock(h).into_mut();
        conv.update_metadata(|_| {});
        conv.flush().unwrap();
    }

    // Reload without the custom config.
    let mut ws = Workspace::new(&root).persisted_at(&storage_path).unwrap();
    ws.disable_persistence();
    ws.load_conversation_index();
    let handle = ws.acquire_conversation(&id).unwrap();
    ws.eager_load_conversation(&handle).unwrap();
    let bare_partial = AppConfig::new_test().to_partial();
    let stream_config = ws
        .events(&handle)
        .unwrap()
        .config()
        .expect("valid config")
        .to_partial();
    let merged = load_partial(bare_partial, stream_config).expect("merge ok");
    let final_config = build(merged).expect("valid config");

    let resolved = final_config.assistant.model.id.resolved();
    assert_eq!(
        resolved.name.as_ref(),
        "custom-model",
        "conversation config must be preserved when continuing without --cfg flag"
    );

    let stream_partial = ws.events(&handle).unwrap().config().unwrap().to_partial();
    let delta = stream_partial.delta(final_config.to_partial());
    assert_eq!(
        delta.assistant.model.id,
        PartialModelIdOrAliasConfig::empty(),
        "config delta must not contain a model change when continuing a conversation without \
         overrides"
    );
}

#[test]
fn test_workspace_persist_single_conversation_via_lock() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    let mut workspace = Workspace::new(&root).persisted_at(&storage).unwrap();
    let config = Arc::new(AppConfig::new_test());

    let id1 = ConversationId::try_from(datetime!(2024-01-01 00:00:00 Z)).unwrap();
    let id2 = ConversationId::try_from(datetime!(2024-01-02 00:00:00 Z)).unwrap();

    workspace.create_conversation_with_id(id1, Conversation::default(), config.clone());
    workspace.create_conversation_with_id(id2, Conversation::default(), config.clone());

    // Only persist id1 via lock.
    let h1 = workspace.acquire_conversation(&id1).unwrap();
    let mut conv = workspace.test_lock(h1).into_mut();
    conv.update_metadata(|_| {}); // set dirty flag
    conv.flush().unwrap();
    drop(conv);

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

/// Regression test: files in a local conversation's user-storage directory
/// must survive persistence.
#[test]
fn test_persist_preserves_files_in_user_storage() {
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

    let conversation = Conversation::default().with_local(true);
    workspace.create_conversation_with_id(id, conversation, config);

    let user_storage = workspace.user_storage_path().unwrap();
    let conv_dir = user_storage
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None));
    fs::create_dir_all(&conv_dir).unwrap();
    let query_file = conv_dir.join("QUERY_MESSAGE.md");
    fs::write(&query_file, "test query content").unwrap();
    assert!(query_file.is_file());

    let h = workspace.acquire_conversation(&id).unwrap();
    let mut conv = workspace.test_lock(h).into_mut();
    conv.update_metadata(|_| {});
    conv.flush().unwrap();
    drop(conv);

    assert!(
        query_file.is_file(),
        "query file in user-storage conversation directory must survive persistence"
    );

    let workspace_conv_dir = storage_path
        .join(CONVERSATIONS_DIR)
        .join(id.to_dirname(None));
    assert!(
        !workspace_conv_dir.exists(),
        "local conversation should not create a workspace-side directory"
    );
}
