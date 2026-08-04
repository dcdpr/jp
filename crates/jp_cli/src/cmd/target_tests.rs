use std::sync::Arc;

use camino::Utf8PathBuf;
use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::{AppConfig, conversation::DefaultConversationId};
use jp_conversation::Conversation;
use jp_workspace::{
    Workspace,
    session::{SessionId, SessionSource},
};

use super::*;

fn workspace_with_conversation() -> (Workspace, ConversationId) {
    let mut ws = Workspace::new(Utf8PathBuf::new());
    let config = Arc::new(AppConfig::new_test());
    let id = ws.create_conversation(Conversation::default(), config);
    (ws, id)
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

/// A workspace whose session activated `previous` and then `active`, so the two
/// session-scoped keywords resolve to different conversations.
fn workspace_with_session_history() -> (Workspace, Session, ConversationId, ConversationId) {
    let mut ws = Workspace::new(Utf8PathBuf::new());
    let config = Arc::new(AppConfig::new_test());
    let previous = make_id(1000);
    let active = make_id(2000);
    ws.create_conversation_with_id(previous, Conversation::default(), Arc::clone(&config));
    ws.create_conversation_with_id(active, Conversation::default(), config);

    let session = Session {
        id: SessionId::new("jp-cli-target-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let at = Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap();
    ws.record_session_activation(&session, previous, at)
        .unwrap();
    ws.record_session_activation(&session, active, at).unwrap();

    (ws, session, previous, active)
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
    assert!(!ConversationTarget::Recent.is_archived());
    assert!(!ConversationTarget::Newest.is_archived());
    assert!(!ConversationTarget::Picker(PickerFilter::default()).is_archived());
}

#[test]
fn archived_keyword_errors_when_no_archived_conversations() {
    let (ws, _) = workspace_with_conversation();
    // Live conversations exist, but none are archived.
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

#[test]
fn parse_active_keyword() {
    assert_eq!(
        ConversationTarget::parse("active"),
        ConversationTarget::SessionActive
    );
    assert_eq!(
        ConversationTarget::parse("."),
        ConversationTarget::SessionActive
    );
}

#[test]
fn parse_all_live_keyword() {
    assert_eq!(
        ConversationTarget::parse("+live"),
        ConversationTarget::AllLive
    );
    assert_eq!(ConversationTarget::parse("+l"), ConversationTarget::AllLive);
}

/// Bare `+` is reserved for a future "every conversation, either partition"
/// keyword, so it must not resolve to the live-only set.
#[test]
fn bare_plus_is_not_a_keyword() {
    assert!(matches!(
        ConversationTarget::parse("+"),
        ConversationTarget::Picker(_)
    ));
}

#[test]
fn active_requires_session_and_all_live_does_not() {
    assert!(ConversationTarget::SessionActive.requires_session());
    assert!(!ConversationTarget::AllLive.requires_session());
}

#[test]
fn active_and_all_live_keyword_names() {
    assert_eq!(
        ConversationTarget::SessionActive.keyword_name(),
        Some("active")
    );
    assert_eq!(ConversationTarget::AllLive.keyword_name(), Some("+live"));
}

#[test]
fn active_resolves_to_the_head_of_the_session_history() {
    let (ws, session, previous, active) = workspace_with_session_history();

    assert_eq!(
        ConversationTarget::SessionActive
            .resolve(&ws, Some(&session))
            .unwrap(),
        vec![active]
    );

    // `session` / `s` is the entry *behind* the active one. Asserted here so a
    // change that collapses the two keywords fails loudly.
    assert_eq!(
        ConversationTarget::SessionPrevious
            .resolve(&ws, Some(&session))
            .unwrap(),
        vec![previous]
    );
}

#[test]
fn active_without_session_errors() {
    let (ws, _) = workspace_with_conversation();
    assert!(
        ConversationTarget::SessionActive
            .resolve(&ws, None)
            .is_err()
    );
}

#[test]
fn active_errors_when_session_has_no_history() {
    let (ws, _) = workspace_with_conversation();
    let session = Session {
        id: SessionId::new("jp-cli-target-test-empty").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    assert!(
        ConversationTarget::SessionActive
            .resolve(&ws, Some(&session))
            .is_err()
    );
}

#[test]
fn all_live_resolves_to_every_live_conversation() {
    let (ws, session, previous, active) = workspace_with_session_history();

    let mut ids = ConversationTarget::AllLive
        .resolve(&ws, Some(&session))
        .unwrap();
    ids.sort_unstable();
    assert_eq!(ids, vec![previous, active]);
}

#[test]
fn all_live_empty_workspace_errors() {
    let ws = Workspace::new(Utf8PathBuf::new());
    assert!(ConversationTarget::AllLive.resolve(&ws, None).is_err());
}

// --- Picker label formatting ------------------------------------------------

fn picker_row(id_secs: u64, time_str: &str, title: Option<&str>) -> PickerRow {
    use chrono::Utc;
    let id = ConversationId::try_from(
        chrono::DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(id_secs),
    )
    .unwrap();
    PickerRow {
        id,
        time_str: time_str.to_string(),
        title: title.map(str::to_owned),
    }
}

#[test]
fn format_picker_label_id_only() {
    let row = picker_row(1000, "", None);
    let label = format_picker_label(&row, 0);
    assert_eq!(label, row.id.to_string());
}

#[test]
fn format_picker_label_with_title_only() {
    let row = picker_row(1000, "", Some("My title"));
    let label = format_picker_label(&row, 0);
    assert_eq!(label, format!("{}  My title", row.id));
}

#[test]
fn format_picker_label_with_time_only() {
    let row = picker_row(1000, "3 days ago", None);
    let label = format_picker_label(&row, 10);
    assert_eq!(label, format!("{}  3 days ago", row.id));
}

#[test]
fn format_picker_label_with_time_and_title_pads_time_column() {
    // The time column is right-padded to the given width so titles align.
    let row = picker_row(1000, "3 days ago", Some("My title"));
    let label = format_picker_label(&row, 14);
    // 14 chars for time column → "3 days ago    " (4 trailing spaces).
    assert_eq!(label, format!("{}  3 days ago      My title", row.id));
}

#[test]
fn format_relative_none_is_empty_string() {
    assert_eq!(format_relative(None), "");
}

#[test]
fn format_relative_future_is_now() {
    use chrono::{Duration, Utc};
    let future = Utc::now() + Duration::hours(1);
    assert_eq!(format_relative(Some(future)), "now");
}

#[test]
fn format_relative_past_uses_timeago_words() {
    use chrono::{Duration, Utc};
    let past = Utc::now() - Duration::days(3);
    let out = format_relative(Some(past));
    // timeago's exact wording can vary across versions, but "day" should
    // always appear for a 3-day-old timestamp.
    assert!(
        out.contains("day"),
        "expected 'day' in relative time, got: {out}"
    );
}
