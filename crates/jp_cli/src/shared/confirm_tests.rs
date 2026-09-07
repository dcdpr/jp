use std::sync::Arc;

use chrono::{DateTime, TimeZone as _, Utc};
use clap::Parser as _;
use jp_config::AppConfig;
use jp_conversation::{Conversation, Labels};
use jp_printer::{OutputFormat, Printer};
use jp_workspace::{ConversationLock, LockResult, Workspace};
use tokio::runtime::Runtime;

use super::*;
use crate::{Globals, bootstrap::ExecutionContext};

#[derive(Debug, clap::Parser)]
struct TestCli {
    #[command(flatten)]
    confirm: ConfirmFlag,
}

fn preference(args: &[&str]) -> Option<bool> {
    let mut argv = vec!["test"];
    argv.extend_from_slice(args);
    TestCli::try_parse_from(argv).unwrap().confirm.preference()
}

#[test]
fn no_flag_defers_to_command_default() {
    assert_eq!(preference(&[]), None);
}

#[test]
fn confirm_forces_prompt() {
    assert_eq!(preference(&["--confirm"]), Some(true));
}

#[test]
fn no_confirm_skips_prompt() {
    assert_eq!(preference(&["--no-confirm"]), Some(false));
}

#[test]
fn yes_and_short_are_aliases_for_no_confirm() {
    assert_eq!(preference(&["--yes"]), Some(false));
    assert_eq!(preference(&["-y"]), Some(false));
}

#[test]
fn last_flag_on_the_line_wins() {
    assert_eq!(preference(&["--confirm", "--no-confirm"]), Some(false));
    assert_eq!(preference(&["--no-confirm", "--confirm"]), Some(true));
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

fn workspace_with(id: ConversationId, conversation: Conversation) -> Workspace {
    let mut workspace = Workspace::in_memory("/tmp/jp-cli-confirm-test");
    workspace.create_conversation_with_id(id, conversation, Arc::new(AppConfig::new_test()));
    workspace
}

fn lock_for(workspace: &Workspace, id: ConversationId) -> ConversationLock {
    let handle = workspace.acquire_conversation(&id).unwrap();
    let LockResult::Acquired(lock) = workspace.lock_conversation(handle, None).unwrap() else {
        panic!("nothing else holds this conversation");
    };
    lock
}

/// A conversation with every row the prompt can show, on fixed values.
fn titled_conversation() -> Conversation {
    Conversation {
        title: Some("Rework the config pipeline".to_owned()),
        pinned_at: Some(Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap()),
        labels: Labels::from_iter([("crate", vec!["jp_config"])]),
        last_activated_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
        ..Conversation::default()
    }
}

/// The heading and the title are two different things: the heading says what is
/// about to happen, the `Title` row says which conversation it happens to.
/// Sharing one slot cost the title, since the heading is written second.
#[test]
fn details_show_the_title_as_a_row_beneath_the_heading() {
    let id = make_id(1000);
    let workspace = workspace_with(id, titled_conversation());
    let lock = lock_for(&workspace, id);

    let details =
        action_details(&lock, Some(id), false).with_heading("Removing conversation jp-c10000");

    assert_eq!(
        details.to_string(),
        "Removing conversation jp-c10000\n\n             ID  jp-c10000\n          Title  Rework \
         the config pipeline\n Last Activated  Currently Active\n         Pinned  Yes\n          \
         Local  No\n         Labels\n                 1. crate\n                     jp_config"
    );
}

/// A conversation is "Currently Active" only when the session activated *it*.
/// With no session, or one pointing elsewhere, the row carries the activation
/// timestamp and the payload reports `active: false`.
///
/// Asserted by claim rather than on the whole block: the timestamp renders
/// relative to `Utc::now()` ("3 months ago (…)"), which no fixed input pins.
#[test]
fn a_conversation_the_session_did_not_activate_is_not_currently_active() {
    let id = make_id(1000);
    let elsewhere = make_id(2000);
    let workspace = workspace_with(id, titled_conversation());
    let lock = lock_for(&workspace, id);

    for active_id in [None, Some(elsewhere)] {
        let details = action_details(&lock, active_id, false);

        assert!(
            !details.to_string().contains("Currently Active"),
            "active_id {active_id:?} must not read as active:\n{details}"
        );
        assert_eq!(
            details.json()["active"],
            serde_json::json!(false),
            "active_id {active_id:?} must report `active: false`"
        );
    }
}

/// The conversation the session activated reports itself as such in both views.
#[test]
fn the_session_active_conversation_reports_itself_as_active() {
    let id = make_id(1000);
    let workspace = workspace_with(id, titled_conversation());
    let lock = lock_for(&workspace, id);

    let details = action_details(&lock, Some(id), false);

    assert!(
        details
            .to_string()
            .contains(" Last Activated  Currently Active")
    );
    assert_eq!(details.json()["active"], serde_json::json!(true));
}

/// Pinning is the strongest reason to stop and read, so the prompt says so
/// rather than leaving the user to infer it from the wording of the question.
#[test]
fn details_report_an_unpinned_conversation_as_unpinned() {
    let id = make_id(2000);
    let workspace = workspace_with(id, Conversation {
        pinned_at: None,
        ..titled_conversation()
    });
    let lock = lock_for(&workspace, id);

    let rendered = action_details(&lock, None, false).to_string();
    assert!(
        rendered.contains(" Pinned  No"),
        "expected an explicit `Pinned  No` row, got:\n{rendered}"
    );
}

fn test_ctx(id: ConversationId, conversation: Conversation) -> Ctx {
    let workspace = workspace_with(id, conversation);
    let (printer, _, _) = Printer::memory(OutputFormat::TextPretty);

    Ctx::new(
        ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    )
}

/// The prompt reads the controlling terminal, not stdin, so a run with nobody
/// watching would stop here and wait for a keystroke that never comes.
/// Answering on the user's behalf is not an option either: a removal cannot be
/// undone, and reading the silence as a decline would have a bulk archive
/// report success having archived nothing.
#[test]
fn a_prompt_nobody_can_answer_fails_and_names_the_way_through() {
    let id = make_id(1000);
    let mut ctx = test_ctx(id, Conversation::default());
    ctx.term.interactive = false;

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let LockResult::Acquired(lock) = ctx.workspace.lock_conversation(handle, None).unwrap() else {
        panic!("nothing else holds this conversation");
    };

    let message = |action| {
        let error = confirm_conversation_action(&ctx, action, &lock, None)
            .expect_err("an action nobody can confirm must not be assumed either way");
        let crate::error::Error::Command(error) = error else {
            panic!("expected a command error, got: {error}");
        };
        error.message.expect("the diagnostic carries a message")
    };

    assert_eq!(
        message(ConversationAction::Remove),
        "removing conversation jp-c10000 needs a confirmation and nobody is available to give \
         one; pass --no-confirm to remove it without asking"
    );
    assert_eq!(
        message(ConversationAction::Archive),
        "archiving conversation jp-c10000 needs a confirmation and nobody is available to give \
         one; pass --no-confirm to archive it without asking"
    );
}
