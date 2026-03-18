use std::{collections::HashMap, fs, time::Duration};

use camino_tempfile::tempdir;
use datetime_literal::datetime;
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
fn test_load_succeeds_when_no_conversations_exist() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("root");
    let storage = root.join("storage");

    let missing_id = ConversationId::try_from(datetime!(2024-06-01 00:00:00 Z)).unwrap();

    fs::create_dir_all(&storage).unwrap();
    write_conversations_metadata_to_disk(&storage, &missing_id);

    let mut workspace = Workspace::new(&root).persisted_at(&storage).unwrap();
    workspace.disable_persistence();

    // A fresh workspace with no conversations on disk is valid — load
    // should succeed with default state.
    let config = Arc::new(AppConfig::new_test());
    workspace.load_conversations_from_disk(config).unwrap();

    // The active conversation must have an events entry.
    let active_id = workspace.active_conversation_id();
    assert!(workspace.get_events(&active_id).is_some());
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

/// Write a `conversations/metadata.json` pointing to the given active ID.
fn write_conversations_metadata_to_disk(storage: &Utf8Path, active_id: &ConversationId) {
    let meta_path = storage.join(CONVERSATIONS_DIR).join(METADATA_FILE);
    let meta = ConversationsMetadata::new(*active_id);

    write_json(&meta_path, &meta).unwrap();
}
