use std::{fs, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::{Utf8TempDir, tempdir};
use crossterm::style::Stylize as _;
use jp_config::{AppConfig, PartialAppConfig};
use jp_conversation::ConversationId;
use jp_printer::{OutputFormat, Printer};
use jp_storage::{
    backend::{FsStorageBackend, LoadBackend as _},
    validate::validate_conversation_dir,
};

use super::*;

/// The `TRASHED.md` startup validation wrote beside every conversation it moved
/// to the trash.
const AUTO_TRASHED_MD: &str = "# Trashed Conversation

This conversation was moved here because it failed workspace sanitization.
";

const EVENTS: &str = r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request"}]"#;

fn setup() -> (Utf8TempDir, FsStorageBackend, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let backend = FsStorageBackend::new(&tmp.path().join(".jp")).unwrap();
    let conversations = tmp.path().join(".jp").join("conversations");
    fs::create_dir_all(&conversations).unwrap();
    (tmp, backend, conversations)
}

/// Write a trash entry as startup validation left it, holding `files`.
fn trashed(conversations: &Utf8Path, dirname: &str, files: &[(&str, &str)]) -> Utf8PathBuf {
    let dir = conversations.join(".trash").join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("TRASHED.md"), AUTO_TRASHED_MD).unwrap();
    for (name, content) in files {
        fs::write(dir.join(name), content).unwrap();
    }
    dir
}

fn live(conversations: &Utf8Path, dirname: &str) -> Utf8PathBuf {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("metadata.json"), "{}").unwrap();
    fs::write(dir.join("events.json"), EVENTS).unwrap();
    dir
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

fn restore(backend: &FsStorageBackend) -> Vec<Leftover> {
    restore_trash(backend, || Some(Arc::new(AppConfig::new_test())))
}

fn listed(conversations: &Utf8Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(conversations)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

// Whatever an entry holds, it goes back: what fails to load is reported by the
// load-failure report, and what is not a conversation by `jp doctor`.
#[test]
fn moves_every_entry_back() {
    let (_tmp, backend, conversations) = setup();
    trashed(&conversations, "17941219991", &[
        ("metadata.json", "{}"),
        ("events.json", EVENTS),
    ]);
    trashed(&conversations, "17941219993", &[(
        "metadata.json",
        "not json",
    )]);
    trashed(&conversations, "not-an-id", &[("notes", "kept")]);

    assert_eq!(restore(&backend), []);

    assert_eq!(
        fs::read_to_string(conversations.join("17941219991/events.json")).unwrap(),
        EVENTS
    );
    assert_eq!(
        fs::read_to_string(conversations.join("17941219993/metadata.json")).unwrap(),
        "not json"
    );
    assert_eq!(
        fs::read_to_string(conversations.join("not-an-id/notes")).unwrap(),
        "kept"
    );
    for dirname in ["17941219991", "17941219993", "not-an-id"] {
        assert!(!conversations.join(dirname).join("TRASHED.md").exists());
    }
    assert!(!conversations.join(".trash").exists());
}

// A draft older versions left without its conversation comes back as a
// conversation with no turns, created when its ID says, holding the draft.
#[test]
fn a_draft_comes_back_as_a_conversation_with_no_turns() {
    let (_tmp, backend, conversations) = setup();
    trashed(&conversations, "17941219992", &[(
        "QUERY_MESSAGE.md",
        "Reply with: ok",
    )]);

    assert_eq!(restore(&backend), []);

    let dir = conversations.join("17941219992");
    assert_eq!(validate_conversation_dir(&dir), Ok(()));
    assert_eq!(
        fs::read_to_string(dir.join("QUERY_MESSAGE.md")).unwrap(),
        "Reply with: ok"
    );
    let metadata = backend
        .load_conversation_metadata(&id("17941219992"))
        .unwrap();
    assert_eq!(metadata.last_activated_at, id("17941219992").timestamp());
    let stream = backend
        .load_conversation_stream(&id("17941219992"), &PartialAppConfig::empty())
        .unwrap();
    assert_eq!(stream.turn_count(), 0);
}

// An entry holding nothing but its `TRASHED.md` holds nothing of the user's.
#[test]
fn an_empty_entry_is_removed() {
    let (_tmp, backend, conversations) = setup();
    trashed(&conversations, "17941219991", &[]);

    assert_eq!(restore(&backend), []);

    assert_eq!(listed(&conversations), Vec::<String>::new());
}

// A conversation that is live, or archived, already has the ID, so the entry
// comes back as a conversation of its own, the next free tenth of a second
// later, and the live one is untouched.
#[test]
fn an_entry_whose_id_is_taken_comes_back_under_the_next_free_id() {
    let (_tmp, backend, conversations) = setup();
    let taken = live(&conversations, "17941219994-a-title");
    live(&conversations.join(".archive"), "17941219995");
    trashed(&conversations, "17941219994", &[
        ("metadata.json", "{}"),
        ("events.json", "[]"),
    ]);
    trashed(&conversations, "17941219995", &[
        ("metadata.json", "{}"),
        ("events.json", "[]"),
    ]);

    assert_eq!(restore(&backend), []);

    assert_eq!(
        fs::read_to_string(taken.join("events.json")).unwrap(),
        EVENTS
    );
    assert_eq!(listed(&conversations), [
        ".archive",
        "17941219994-a-title",
        "17941219996",
        "17941219997",
    ]);
    assert_eq!(
        fs::read_to_string(conversations.join("17941219996/events.json")).unwrap(),
        "[]"
    );
}

// Older versions named a second entry for one ID `<id>-1`; both come back.
#[test]
fn two_entries_for_one_id_both_come_back() {
    let (_tmp, backend, conversations) = setup();
    trashed(&conversations, "17941219994", &[
        ("metadata.json", "{}"),
        ("events.json", "[]"),
    ]);
    trashed(&conversations, "17941219994-1", &[
        ("metadata.json", "{}"),
        ("events.json", EVENTS),
    ]);

    assert_eq!(restore(&backend), []);

    assert_eq!(listed(&conversations), ["17941219994", "17941219995"]);
    assert_eq!(
        fs::read_to_string(conversations.join("17941219995/events.json")).unwrap(),
        EVENTS
    );
}

// A directory that is not a conversation is moved back under its own name, so
// an existing directory of that name keeps it in the trash.
#[test]
fn an_entry_whose_name_is_taken_is_left_and_reported() {
    let (_tmp, backend, conversations) = setup();
    fs::create_dir_all(conversations.join("not-an-id")).unwrap();
    let entry = trashed(&conversations, "not-an-id", &[("notes", "kept")]);

    assert_eq!(restore(&backend), [Leftover {
        entry: entry.clone(),
        reason: LeftoverReason::NameTaken,
    }]);

    assert_eq!(fs::read_to_string(entry.join("notes")).unwrap(), "kept");
}

#[test]
fn a_root_without_a_trash_is_left_alone() {
    let (_tmp, backend, conversations) = setup();

    assert_eq!(
        restore_trash(&backend, || panic!("no config is needed without a trash")),
        []
    );

    assert_eq!(listed(&conversations), Vec::<String>::new());
}

// The config is only built when a draft needs one.
#[test]
fn no_config_is_built_without_a_draft() {
    let (_tmp, backend, conversations) = setup();
    trashed(&conversations, "17941219991", &[("events.json", "[]")]);

    assert_eq!(restore_trash(&backend, || panic!("no config is needed")), [
    ]);
}

// User-local storage has a trash of its own.
#[test]
fn restores_the_trash_of_every_storage_root() {
    let tmp = tempdir().unwrap();
    let backend = FsStorageBackend::new(&tmp.path().join(".jp"))
        .unwrap()
        .with_user_storage(&tmp.path().join("user"), None, "abc")
        .unwrap();
    let [workspace, user] = <[Utf8PathBuf; 2]>::try_from(backend.conversations_dirs()).unwrap();
    trashed(&workspace, "17941219991", &[("events.json", EVENTS)]);
    trashed(&user, "17941219992", &[("events.json", EVENTS)]);

    assert_eq!(restore(&backend), []);

    assert!(workspace.join("17941219991/events.json").is_file());
    assert!(user.join("17941219992/events.json").is_file());
}

fn leftover() -> Leftover {
    Leftover {
        entry: Utf8PathBuf::from("/ws/.jp/conversations/.trash/not-an-id"),
        reason: LeftoverReason::NameTaken,
    }
}

#[test]
fn nothing_left_renders_nothing() {
    assert_eq!(render_text(&[]), None);
    assert_eq!(render_json(&[], false).unwrap(), Vec::<String>::new());
}

#[test]
fn a_leftover_is_listed_with_its_reason() {
    let failed = Leftover {
        entry: Utf8PathBuf::from("/ws/.jp/conversations/.trash/17941219991"),
        reason: LeftoverReason::Failed("permission denied".to_owned()),
    };

    let expected = [
        format!(
            "{} could not be restored from the trash:",
            "2 entries".bold().yellow()
        ),
        String::new(),
        format!(
            "- {}: a directory with this name already exists",
            "/ws/.jp/conversations/.trash/not-an-id".bold().yellow()
        ),
        format!(
            "- {}: permission denied",
            "/ws/.jp/conversations/.trash/17941219991".bold().yellow()
        ),
        String::new(),
        "Move or remove them by hand.".to_owned(),
    ];
    assert_eq!(
        render_text(&[leftover(), failed]),
        Some(expected.join("\n"))
    );
}

#[test]
fn a_leftover_is_one_json_record() {
    assert_eq!(render_json(&[leftover()], false).unwrap(), [concat!(
        r#"{"level":"warning","code":"trash_entry_not_restored","#,
        r#""message":"trash entry could not be restored","#,
        r#""context":{"entry":{"location":"/ws/.jp/conversations/.trash/not-an-id"}},"#,
        r#""causes":[{"code":"name_taken","#,
        r#""message":"a directory with this name already exists"}]}"#,
    )]);
}

#[test]
fn the_report_prints_when_it_drops() {
    let (printer, out, err) = Printer::memory(OutputFormat::Text);

    drop(notice(printer.clone(), &[leftover()]));

    printer.flush();

    assert_eq!(out.lock().as_str(), "");
    assert_eq!(
        err.lock().as_str(),
        "1 entry could not be restored from the trash:\n\n- \
         /ws/.jp/conversations/.trash/not-an-id: a directory with this name already \
         exists\n\nMove or remove it by hand.\n"
    );
}
