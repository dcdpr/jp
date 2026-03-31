use std::sync::Arc;

use camino::Utf8PathBuf;
use jp_config::{AppConfig, conversation::DefaultConversationId};
use jp_conversation::Conversation;
use jp_workspace::Workspace;

use super::*;

fn workspace_with_conversation() -> (Workspace, ConversationId) {
    let mut ws = Workspace::new(Utf8PathBuf::new());
    let config = Arc::new(AppConfig::new_test());
    let id = ws.create_conversation(Conversation::default(), config);
    (ws, id)
}

#[test]
fn ask_returns_none() {
    let (ws, _) = workspace_with_conversation();
    assert_eq!(
        resolve_default_id(DefaultConversationId::Ask, &ws, None),
        None
    );
}

#[test]
fn last_activated_resolves() {
    let (ws, id) = workspace_with_conversation();
    assert_eq!(
        resolve_default_id(DefaultConversationId::LastActivated, &ws, None),
        Some(id)
    );
}

#[test]
fn last_created_resolves() {
    let (ws, id) = workspace_with_conversation();
    assert_eq!(
        resolve_default_id(DefaultConversationId::LastCreated, &ws, None),
        Some(id)
    );
}

#[test]
fn last_activated_empty_workspace_returns_none() {
    let ws = Workspace::new(Utf8PathBuf::new());
    assert_eq!(
        resolve_default_id(DefaultConversationId::LastActivated, &ws, None),
        None
    );
}

#[test]
fn previous_without_session_returns_none() {
    let (ws, _) = workspace_with_conversation();
    assert_eq!(
        resolve_default_id(DefaultConversationId::Previous, &ws, None),
        None
    );
}

#[test]
fn specific_id_resolves() {
    let (ws, id) = workspace_with_conversation();
    assert_eq!(
        resolve_default_id(DefaultConversationId::Id(id.to_string()), &ws, None),
        Some(id)
    );
}

#[test]
fn invalid_id_returns_none() {
    let (ws, _) = workspace_with_conversation();
    assert_eq!(
        resolve_default_id(
            DefaultConversationId::Id("not-a-valid-id".into()),
            &ws,
            None
        ),
        None
    );
}
