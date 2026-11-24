use std::sync::Arc;

use chrono::{DateTime, TimeZone as _, Utc};
use jp_config::AppConfig;
use jp_conversation::{Conversation, Labels};
use jp_printer::{OutputFormat, Printer};
use jp_storage::{
    backend::StoredConversation,
    validate::{ConversationPart, Position, ReadError, StorageLocation, ValidationError},
};
use jp_workspace::{ConversationLock, LockResult, Workspace};
use tokio::runtime::Runtime;

use super::*;
use crate::{Globals, bootstrap::ExecutionContext, cli::parse_args};

fn preference(args: &[&str]) -> Option<bool> {
    parse_args::<ConfirmFlag>(args).unwrap().preference()
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

/// Removal hard-deletes the conversation from both storage roots, so the user
/// has to be told before answering rather than after pressing `?`.
/// Archiving is reversible with `jp c unarchive`, so it carries no caution.
#[test]
fn only_the_irreversible_action_cautions_the_user() {
    assert_eq!(
        ConversationAction::Remove.caution(),
        Some("this action cannot be undone")
    );
    assert_eq!(ConversationAction::Archive.caution(), None);
}

/// `n` and Esc are both the user answering the question, so the run carries on
/// to the next conversation.
/// Ctrl-C is not an answer: `inquire` reads it in raw mode, so no SIGINT is
/// raised and this is the only thing that can end a bulk run part-way through.
#[test]
fn esc_declines_but_ctrl_c_ends_the_run() {
    assert_eq!(decide(Ok('y')).unwrap(), Answer::Yes);
    assert_eq!(decide(Ok('n')).unwrap(), Answer::No);
    assert_eq!(
        decide(Err(InquireError::OperationCanceled)).unwrap(),
        Answer::No,
        "Esc declines this one only"
    );

    let error = decide(Err(InquireError::OperationInterrupted))
        .expect_err("Ctrl-C must not read as a decline");
    let crate::error::Error::Command(error) = error else {
        panic!("expected a command error, got: {error}");
    };
    assert_eq!(error.code.get(), 130, "the shell's status for a SIGINT");
    assert_eq!(error.message, None, "Ctrl-C needs no diagnostic");
}

#[test]
fn the_capitals_answer_for_every_remaining_conversation() {
    assert_eq!(decide(Ok('Y')).unwrap(), Answer::YesToAll);
    assert_eq!(decide(Ok('N')).unwrap(), Answer::NoToAll);
}

#[test]
fn yes_and_no_to_all_are_offered_only_when_more_follow() {
    let prompt = ConversationAction::Remove.prompt();
    let keys = |more| {
        options(&prompt, more)
            .into_iter()
            .map(|option| option.key)
            .collect::<String>()
    };

    assert_eq!(keys(true), "yYnN");
    assert_eq!(keys(false), "yn");
}

fn never_asked(_more: bool) -> std::result::Result<Answer, ()> {
    panic!("asked a question the answers already settled")
}

#[test]
fn yes_answers_only_this_conversation() {
    let mut answers = Answers::default();
    let mut asked = vec![];

    for more in [true, true, false] {
        let accepted = answers
            .decide(more, |more| {
                asked.push(more);
                Ok::<_, ()>(Answer::Yes)
            })
            .unwrap();
        assert!(accepted);
    }

    assert_eq!(asked, [true, true, false]);
    assert_eq!(answers.rest(), None);
}

#[test]
fn yes_to_all_answers_every_remaining_conversation_without_asking() {
    let mut answers = Answers::default();

    let first = answers.decide(true, |_| Ok::<_, ()>(Answer::YesToAll));
    let second = answers.decide(true, never_asked);
    let last = answers.decide(false, never_asked);

    assert_eq!((first, second, last), (Ok(true), Ok(true), Ok(true)));
    assert_eq!(answers.rest(), Some(true));
}

#[test]
fn no_to_all_declines_every_remaining_conversation_without_asking() {
    let mut answers = Answers::default();

    let first = answers.decide(true, |_| Ok::<_, ()>(Answer::NoToAll));
    let second = answers.decide(false, never_asked);

    assert_eq!((first, second), (Ok(false), Ok(false)));
    assert_eq!(answers.rest(), Some(false));
}

#[test]
fn answers_that_start_at_yes_to_all_never_ask() {
    let mut answers = Answers::yes_to_all();

    let accepted = answers.decide(true, never_asked);

    assert_eq!(accepted, Ok(true));
}

/// A terminal that cannot be read is a broken prompt, not a choice the user
/// made, so it keeps its cause instead of silently skipping the conversation.
#[test]
fn a_broken_prompt_is_reported_rather_than_treated_as_a_decline() {
    assert!(matches!(
        decide(Err(InquireError::NotTTY)),
        Err(crate::error::Error::Inquire(InquireError::NotTTY))
    ));
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

/// The block is written by the prompt widget, which draws without the printer's
/// filter, so the title is filtered on its way in.
/// The `ESC` that opens each sequence goes, and what follows it is left as
/// plain text.
#[test]
fn details_show_a_title_as_plain_text() {
    let id = make_id(1000);
    let workspace = workspace_with(id, Conversation {
        title: Some("Rework\x1b[2J the\r config".to_owned()),
        ..titled_conversation()
    });
    let lock = lock_for(&workspace, id);

    let details =
        action_details(&lock, Some(id), false).with_heading("Removing conversation jp-c10000");

    assert_eq!(
        details.to_string(),
        "Removing conversation jp-c10000\n\n             ID  jp-c10000\n          Title  \
         Rework[2J the config\n Last Activated  Currently Active\n         Pinned  Yes\n          \
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

fn stored(
    title: Option<&str>,
    state: std::result::Result<(), ValidationError>,
) -> StoredConversation {
    StoredConversation {
        id: make_id(1000),
        locations: vec![StorageLocation::new("/ws/.jp/conversations/10000")],
        state,
        title: title.map(str::to_owned),
        repair: None,
        archived: false,
    }
}

#[test]
fn unloaded_details_show_the_title_status_and_every_location() {
    let mut conversation = stored(
        Some("Fix the loader"),
        Err(ValidationError::Unreadable {
            part: ConversationPart::Metadata,
            location: StorageLocation::labelled(
                "/ws/.jp/conversations/10000/metadata.json",
                "metadata.json",
            ),
            error: ReadError::Malformed {
                format: Some("JSON"),
                message: "expected ident at line 1 column 2".to_owned(),
                position: Some(Position { line: 1, column: 2 }),
            },
        }),
    );
    conversation
        .locations
        .push(StorageLocation::new("/user/conversations/10000-a-title"));

    let expected = [
        "Removing conversation jp-c10000",
        "",
        "        ID  jp-c10000",
        "     Title  Fix the loader",
        "    Status  unable to load: metadata.json: invalid JSON at line 1, column 2",
        " Locations",
        "            1. /ws/.jp/conversations/10000",
        "            2. /user/conversations/10000-a-title",
    ];
    assert_eq!(
        unloaded_details(&conversation, false).to_string(),
        expected.join("\n")
    );
}

#[test]
fn unloaded_details_leave_out_what_is_not_known() {
    let expected = [
        "Removing conversation jp-c10000",
        "",
        "        ID  jp-c10000",
        " Locations",
        "            1. /ws/.jp/conversations/10000",
    ];
    assert_eq!(
        unloaded_details(&stored(None, Ok(())), false).to_string(),
        expected.join("\n")
    );
}

#[test]
fn unloaded_details_show_a_title_as_plain_text() {
    let expected = [
        "Removing conversation jp-c10000",
        "",
        "        ID  jp-c10000",
        "     Title  Fix[2J the loader",
        " Locations",
        "            1. /ws/.jp/conversations/10000",
    ];
    assert_eq!(
        unloaded_details(&stored(Some("Fix\x1b[2J the\r loader"), Ok(())), false).to_string(),
        expected.join("\n")
    );
}

#[test]
fn an_unloaded_removal_nobody_can_confirm_fails() {
    let id = make_id(1000);
    let mut ctx = test_ctx(id, Conversation::default());
    ctx.term.interactive = false;

    let error = confirm_unloaded_removal(&ctx, &stored(None, Ok(())), false)
        .expect_err("a removal nobody can confirm must not be assumed");
    let crate::error::Error::Command(error) = error else {
        panic!("expected a command error, got: {error}");
    };

    assert_eq!(
        error.message.as_deref(),
        Some(
            "removing conversation jp-c10000 needs a confirmation and nobody is available to give \
             one; pass --no-confirm to remove it without asking"
        )
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
        let error = confirm_conversation_action(&ctx, action, &lock, None, false)
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
