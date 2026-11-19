use std::{fs, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use crossterm::style::Stylize as _;
use jp_config::AppConfig;
use jp_conversation::ConversationId;
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Globals,
    bootstrap::ExecutionContext,
    cmd::target::{ConversationTarget, PickerFilter},
};

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

fn targets(dirnames: &[&str]) -> PositionalIds<false, true> {
    PositionalIds::from_targets(
        dirnames
            .iter()
            .map(|dirname| ConversationTarget::Id(id(dirname)))
            .collect(),
    )
}

struct Fixture {
    _tmp: Utf8TempDir,
    ctx: Ctx,
    out: SharedBuffer,
    err: SharedBuffer,
    conversations: Utf8PathBuf,
}

fn fixture(persist: bool) -> Fixture {
    let tmp = camino_tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let fs = Arc::new(FsStorageBackend::new(&root.join(".jp")).unwrap());
    let conversations = root.join(".jp").join("conversations");
    std::fs::create_dir_all(&conversations).unwrap();

    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let workspace = Workspace::in_memory(&root).with_backend(Arc::clone(&fs));
    let ctx = Ctx::new(
        ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(fs),
        Runtime::new().unwrap(),
        Globals {
            persist,
            ..Globals::default()
        },
        AppConfig::new_test(),
        None,
        printer,
    );

    Fixture {
        _tmp: tmp,
        ctx,
        out,
        err,
        conversations,
    }
}

fn write_dir(conversations: &Utf8Path, dirname: &str, files: &[(&str, &str)]) {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        fs::write(dir.join(name), content).unwrap();
    }
}

fn plain(buffer: &SharedBuffer) -> String {
    strip_ansi_escapes::strip_str(buffer.lock().as_str())
}

// The command exists for the conversations JP cannot load, so one that loads
// and one that does not are trashed alike.
#[test]
fn trashes_every_id_it_is_given() {
    let mut f = fixture(true);
    write_dir(&f.conversations, "17941219991", &[
        ("metadata.json", "{}"),
        ("events.json", "[]"),
    ]);
    write_dir(&f.conversations, "17941219992", &[(
        "metadata.json",
        "not json",
    )]);

    Trash {
        target: targets(&["17941219991", "17941219992"]),
    }
    .run(&mut f.ctx)
    .unwrap();
    f.ctx.printer.flush();

    assert_eq!(
        plain(&f.out),
        "Conversation jp-c17941219991 moved to the trash.\nConversation jp-c17941219992 moved to \
         the trash.\n"
    );
    assert_eq!(plain(&f.err), "");
    let trash = f.conversations.join(".trash");
    assert!(trash.join("17941219991").is_dir());
    assert!(trash.join("17941219992").is_dir());
}

// One bad ID does not stop the rest, and the run still fails.
#[test]
fn reports_each_id_it_could_not_trash() {
    let mut f = fixture(true);
    write_dir(&f.conversations, "17941219991", &[("events.json", "[]")]);

    let error = Trash {
        target: targets(&["17941219990", "17941219991"]),
    }
    .run(&mut f.ctx)
    .unwrap_err();
    f.ctx.printer.flush();

    assert_eq!(
        plain(&f.out),
        "Conversation jp-c17941219991 moved to the trash.\n"
    );
    assert_eq!(
        plain(&f.err),
        "Conversation jp-c17941219990 not moved to the trash: no conversation with this ID.\n"
    );
    assert_eq!(
        error.message.as_deref().map(strip_ansi_escapes::strip_str),
        Some(
            "Failed to move 1 of 2 conversations to the trash:\n\njp-c17941219990:  no \
             conversation with this ID"
                .to_owned()
        )
    );
}

#[test]
fn the_failure_summary_lists_every_failure() {
    let summary = failure_summary(Action::Trash, 3, &[
        (id("17941219990"), TrashError::NotFound),
        (id("17941219991"), TrashError::Locked),
    ]);

    assert_eq!(
        strip_ansi_escapes::strip_str(&summary),
        "Failed to move 2 of 3 conversations to the trash:\n\njp-c17941219990:  no conversation \
         with this ID\njp-c17941219991:  in use by another process"
    );
    assert_eq!(
        summary,
        format!(
            "Failed to move 2 of 3 conversations to the trash:\n\n{}:  no conversation with this \
             ID\n{}:  in use by another process",
            "jp-c17941219990".bold().yellow(),
            "jp-c17941219991".bold().yellow(),
        )
    );
}

// Under `--no-persist` the command reports what it would do and moves nothing.
#[test]
fn no_persist_is_a_dry_run() {
    let mut f = fixture(false);
    write_dir(&f.conversations, "17941219991", &[("events.json", "[]")]);

    let error = Trash {
        target: targets(&["17941219990", "17941219991"]),
    }
    .run(&mut f.ctx)
    .unwrap_err();
    f.ctx.printer.flush();

    assert_eq!(
        plain(&f.out),
        "Conversation jp-c17941219991 moved to the trash.\n\nThis command ran under \
         `--no-persist`, no files on disk were modified.\n"
    );
    assert_eq!(
        plain(&f.err),
        "Conversation jp-c17941219990 not moved to the trash: no conversation with this ID.\n"
    );
    assert_eq!(
        error.message.as_deref().map(strip_ansi_escapes::strip_str),
        Some(
            "Failed to move 1 of 2 conversations to the trash:\n\njp-c17941219990:  no \
             conversation with this ID"
                .to_owned()
        )
    );
    assert!(f.conversations.join("17941219991").is_dir());
    assert!(!f.conversations.join(".trash").exists());
}

#[test]
fn no_persist_untrash_moves_nothing() {
    let mut f = fixture(false);
    write_dir(&f.conversations.join(".trash"), "17941219991", &[(
        "events.json",
        "[]",
    )]);

    Untrash {
        target: targets(&["17941219991"]),
    }
    .run(&mut f.ctx)
    .unwrap();
    f.ctx.printer.flush();

    assert_eq!(
        plain(&f.out),
        "Conversation jp-c17941219991 restored from the trash.\n\nThis command ran under \
         `--no-persist`, no files on disk were modified.\n"
    );
    assert!(f.conversations.join(".trash/17941219991").is_dir());
    assert!(!f.conversations.join("17941219991").exists());
}

fn picker(filter: PickerFilter) -> PositionalIds<false, true> {
    PositionalIds::from_targets(vec![ConversationTarget::Picker(filter)])
}

// `?` opens the live-conversation picker. Without a user to answer it, the
// command fails the way every picker does, not with a refusal of the picker.
#[test]
fn trash_picker_needs_a_user() {
    let mut f = fixture(true);
    write_dir(&f.conversations, "17941219991", &[
        ("metadata.json", "{}"),
        ("events.json", "[]"),
    ]);
    f.ctx.term.args.no_interactive = true;

    let error = Trash {
        target: picker(PickerFilter::default()),
    }
    .run(&mut f.ctx)
    .unwrap_err();

    let expected: cmd::Error = Error::NoConversationTarget {
        session: false,
        multi: true,
        allow_new: false,
    }
    .into();
    assert_eq!(error.message, expected.message);
    assert!(f.conversations.join("17941219991").is_dir());
}

// The trash moves live conversations only, so a picker over the archive would
// offer conversations it then cannot move.
#[test]
fn trash_refuses_the_archived_picker() {
    let mut f = fixture(true);

    let error = Trash {
        target: picker(PickerFilter {
            archived: true,
            ..PickerFilter::default()
        }),
    }
    .run(&mut f.ctx)
    .unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some("`jp conversation trash` moves live conversations; archived ones cannot be picked.")
    );
}

// No picker lists the trash yet.
#[test]
fn untrash_refuses_the_picker() {
    let mut f = fixture(true);

    let error = Untrash {
        target: picker(PickerFilter::default()),
    }
    .run(&mut f.ctx)
    .unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some("`jp conversation untrash` takes conversation IDs; it has no picker.")
    );
}

#[test]
fn untrash_moves_trashed_conversations_back() {
    let mut f = fixture(true);
    write_dir(&f.conversations, "17941219991-a-title", &[(
        "events.json",
        "[]",
    )]);
    Trash {
        target: targets(&["17941219991"]),
    }
    .run(&mut f.ctx)
    .unwrap();

    Untrash {
        target: targets(&["17941219991"]),
    }
    .run(&mut f.ctx)
    .unwrap();
    f.ctx.printer.flush();

    assert_eq!(
        plain(&f.out),
        "Conversation jp-c17941219991 moved to the trash.\nConversation jp-c17941219991 restored \
         from the trash.\n"
    );
    assert!(
        f.conversations
            .join("17941219991-a-title")
            .join("events.json")
            .is_file()
    );
}

#[test]
fn untrash_reports_each_id_it_could_not_restore() {
    let mut f = fixture(true);

    let error = Untrash {
        target: targets(&["17941219991"]),
    }
    .run(&mut f.ctx)
    .unwrap_err();
    f.ctx.printer.flush();

    assert_eq!(
        plain(&f.err),
        "Conversation jp-c17941219991 not restored from the trash: not in the trash.\n"
    );
    assert_eq!(
        error.message.as_deref().map(strip_ansi_escapes::strip_str),
        Some(
            "Failed to restore 1 of 1 conversation from the trash:\n\njp-c17941219991:  not in \
             the trash"
                .to_owned()
        )
    );
}
