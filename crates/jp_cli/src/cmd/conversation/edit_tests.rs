use std::{fs, sync::Arc, time::Duration};

use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_config::{AppConfig, types::command::CommandConfigOrString};
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use serde_json::Value;
use tokio::runtime::Runtime;

use super::{Edit, ExpirationDuration};
use crate::{Globals, cmd::conversation_id::PositionalIds, ctx::Ctx};

#[test]
fn parse_now() {
    let dur: ExpirationDuration = "now".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_now_case_insensitive() {
    let dur: ExpirationDuration = "NOW".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);

    let dur: ExpirationDuration = "Now".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_humantime_duration() {
    let dur: ExpirationDuration = "1h".parse().unwrap();
    assert_eq!(dur.0, Duration::from_hours(1));

    let dur: ExpirationDuration = "30m".parse().unwrap();
    assert_eq!(dur.0, Duration::from_mins(30));

    let dur: ExpirationDuration = "0s".parse().unwrap();
    assert_eq!(dur.0, Duration::ZERO);
}

#[test]
fn parse_invalid() {
    let result = "not-a-duration".parse::<ExpirationDuration>();
    assert!(result.is_err());
}

/// A discarded edit must restore files to their pre-edit content, and remove
/// files that did not exist before the edit — so a malformed edit never leaves
/// the conversation in a broken state.
#[test]
fn restore_snapshots_reverts_and_removes() {
    let tmp = tempdir().unwrap();

    // A file that existed before editing: restore brings back the original.
    let existing = tmp.path().join("existing.json");
    fs::write(&existing, "original").unwrap();

    // A file that did not exist before editing.
    let created = tmp.path().join("created.json");

    // Simulate the editor having changed both (the latter into existence).
    fs::write(&existing, "garbage").unwrap();
    fs::write(&created, "garbage").unwrap();

    let snapshots: Vec<(Utf8PathBuf, Option<String>)> = vec![
        (existing.clone(), Some("original".to_owned())),
        (created.clone(), None),
    ];

    Edit::restore_snapshots(&snapshots).unwrap();

    assert_eq!(
        fs::read_to_string(&existing).unwrap(),
        "original",
        "an edited file is reverted to its pre-edit content"
    );
    assert!(
        !created.exists(),
        "a file created by the edit is removed on revert"
    );
}

/// Set up a workspace holding one persisted conversation, with an editor
/// command naming a program that cannot be spawned.
///
/// A bogus command means reaching the editor branch fails loudly, so a test
/// asserting the branch was skipped cannot pass by accident.
fn setup(id: ConversationId) -> (Ctx, Utf8TempDir) {
    let tmp = tempdir().unwrap();
    let fs = Arc::new(FsStorageBackend::new(&tmp.path().join(".jp")).unwrap());
    fs.write_test_conversation(&id, &Conversation::default());

    let mut config = AppConfig::new_test();
    config.editor.cmd = Some(CommandConfigOrString::String(
        "jp-editor-that-does-not-exist".to_owned(),
    ));

    let mut workspace = Workspace::in_memory(tmp.path()).with_backend(fs.clone());
    workspace.load_conversation_index();

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(fs),
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    (ctx, tmp)
}

/// The editor is a child process that exits when a person saves and closes it.
/// With nobody there it blocks forever, so a non-interactive `conversation
/// edit` has to refuse rather than spawn it.
#[test]
fn open_editor_refuses_a_non_interactive_invocation() {
    let id = ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + Duration::from_mins(100),
    )
    .unwrap();
    let (mut ctx, _tmp) = setup(id);

    let edit = || Edit {
        target: PositionalIds::from_targets(vec![]),
        local: None,
        pin: None,
        expires_at: None,
        no_expires_at: false,
        title: None,
        no_title: false,
        events: true,
        metadata: false,
        base_config: false,
    };

    assert!(
        !ctx.term.interactive,
        "a captured stdout is not a terminal, so no user is present"
    );

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let error = edit().run_open_editor(&mut ctx, &[handle]).unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some("Cannot open the editor: nobody is present to close it")
    );
    assert_eq!(error.metadata, vec![(
        "suggestion".to_owned(),
        Value::String(
            "Run this from a terminal, or edit the files directly at the path from `jp \
             conversation path`."
                .to_owned()
        )
    )]);

    // The same call with a user present does reach the editor, which proves
    // the assertion above is pinned on interactivity and not on some other
    // reason to stop early.
    ctx.term.interactive = true;
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let error = edit().run_open_editor(&mut ctx, &[handle]).unwrap_err();

    assert_eq!(error.message.as_deref(), Some("Editor error"));
}
