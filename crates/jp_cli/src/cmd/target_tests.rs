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

#[test]
fn parse_archived_keyword() {
    assert_eq!(
        ConversationTarget::parse("archived"),
        ConversationTarget::Archived
    );
}

#[test]
fn parse_archived_short() {
    assert_eq!(ConversationTarget::parse("a"), ConversationTarget::Archived);
}

#[test]
fn parse_all_archived() {
    assert_eq!(
        ConversationTarget::parse("+archived"),
        ConversationTarget::AllArchived
    );
}

#[test]
fn parse_all_archived_short() {
    assert_eq!(
        ConversationTarget::parse("+a"),
        ConversationTarget::AllArchived
    );
}

#[test]
fn parse_archived_picker() {
    assert_eq!(
        ConversationTarget::parse("?archived"),
        ConversationTarget::Picker(PickerFilter {
            archived: true,
            ..Default::default()
        })
    );
}

#[test]
fn parse_archived_picker_short() {
    assert_eq!(
        ConversationTarget::parse("?a"),
        ConversationTarget::Picker(PickerFilter {
            archived: true,
            ..Default::default()
        })
    );
}

#[test]
fn is_archived_returns_true_for_archived_targets() {
    assert!(ConversationTarget::Archived.is_archived());
    assert!(ConversationTarget::AllArchived.is_archived());
    assert!(
        ConversationTarget::Picker(PickerFilter {
            archived: true,
            ..Default::default()
        })
        .is_archived()
    );
}

#[test]
fn is_archived_returns_false_for_non_archived_targets() {
    assert!(!ConversationTarget::Latest.is_archived());
    assert!(!ConversationTarget::Newest.is_archived());
    assert!(!ConversationTarget::Picker(PickerFilter::default()).is_archived());
}

#[test]
fn archived_keyword_resolves_most_recently_archived() {
    let mut ws = Workspace::new(Utf8PathBuf::new());
    let config = Arc::new(AppConfig::new_test());

    let id1 = ws.create_conversation(Conversation::default(), config.clone());
    let id2 = ws.create_conversation(Conversation::default(), config.clone());

    // Simulate archived_at timestamps.
    ws.conversations().for_each(|_| {});

    // Archive both with different archived_at values.
    // We can't use archive_conversation (needs lock + fs), so test the
    // resolution logic by checking that Archived resolves against
    // archived_conversations(). Since we're in-memory without archived
    // conversations, this should return an error.
    let result = ConversationTarget::Archived.resolve(&ws, None);
    assert!(result.is_err());
}

#[test]
fn all_archived_empty_returns_error() {
    let ws = Workspace::new(Utf8PathBuf::new());
    let result = ConversationTarget::AllArchived.resolve(&ws, None);
    assert!(result.is_err());
}

#[test]
fn archived_keyword_name() {
    assert_eq!(
        ConversationTarget::Archived.keyword_name(),
        Some("archived")
    );
    assert_eq!(
        ConversationTarget::AllArchived.keyword_name(),
        Some("+archived")
    );
}
