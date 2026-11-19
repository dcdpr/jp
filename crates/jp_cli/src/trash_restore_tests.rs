use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::{Utf8TempDir, tempdir};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_storage::{
    backend::{FsStorageBackend, LockBackend as _},
    validate::{ManagedFile, ValidationError},
};

use super::*;

/// The `TRASHED.md` startup validation wrote beside every conversation it moved
/// to the trash.
const AUTO_TRASHED_MD: &str = "# Trashed Conversation

This conversation was moved here because it failed workspace sanitization.

**Error:** missing metadata.json
**Date:** 2026-11-08T07:13:30Z
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
fn auto_trashed(conversations: &Utf8Path, dirname: &str, files: &[(&str, &str)]) -> Utf8PathBuf {
    let dir = conversations.join(".trash").join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("TRASHED.md"), AUTO_TRASHED_MD).unwrap();
    for (name, content) in files {
        fs::write(dir.join(name), content).unwrap();
    }
    dir
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

fn restored_ids(roots: &[RootRestore]) -> Vec<ConversationId> {
    roots
        .iter()
        .flat_map(|root| root.restored.iter().map(|restored| restored.id))
        .collect()
}

// The three shapes that move back: a conversation that loads today, a query
// draft that was never stored, and events without metadata, which is then
// reported as a load failure for the user to repair.
#[test]
fn restores_every_entry_jp_trashed_that_can_come_back() {
    let (_tmp, backend, conversations) = setup();
    auto_trashed(&conversations, "17941219991", &[
        ("metadata.json", "{}"),
        ("events.json", EVENTS),
    ]);
    auto_trashed(&conversations, "17941219992", &[(
        "QUERY_MESSAGE.md",
        "Reply with: ok",
    )]);
    auto_trashed(&conversations, "17941219993", &[("events.json", EVENTS)]);

    let roots = restore_auto_trashed(&backend);

    assert_eq!(restored_ids(&roots), [
        id("17941219991"),
        id("17941219992"),
        id("17941219993")
    ]);
    assert!(roots[0].left.is_empty());

    assert_eq!(
        fs::read_to_string(conversations.join("17941219991/events.json")).unwrap(),
        EVENTS
    );
    assert_eq!(
        fs::read_to_string(conversations.join("17941219992/QUERY_MESSAGE.md")).unwrap(),
        "Reply with: ok"
    );
    assert!(conversations.join("17941219993/events.json").is_file());
    for dirname in ["17941219991", "17941219992", "17941219993"] {
        assert!(!conversations.join(dirname).join("TRASHED.md").exists());
        assert!(!conversations.join(".trash").join(dirname).exists());
    }
}

#[test]
fn leaves_an_entry_that_fails_validation_otherwise() {
    let (_tmp, backend, conversations) = setup();
    let entry = auto_trashed(&conversations, "17941219994", &[
        ("metadata.json", "not json"),
        ("events.json", EVENTS),
    ]);

    let roots = restore_auto_trashed(&backend);

    assert!(roots[0].restored.is_empty());
    assert_eq!(roots[0].left.len(), 1);
    assert_eq!(roots[0].left[0].id, Some(id("17941219994")));
    assert!(matches!(
        roots[0].left[0].reason,
        LeftReason::Invalid(ValidationError::UnreadableFile {
            file: ManagedFile::Metadata,
            ..
        })
    ));
    assert_eq!(
        fs::read_to_string(entry.join("TRASHED.md")).unwrap(),
        AUTO_TRASHED_MD
    );
    assert!(!conversations.join("17941219994").exists());
}

#[test]
fn never_replaces_a_live_conversation() {
    let (_tmp, backend, conversations) = setup();
    let live = conversations.join("17941219994-a-title");
    fs::create_dir_all(&live).unwrap();
    fs::write(live.join("metadata.json"), r#"{"title":"a title"}"#).unwrap();
    fs::write(live.join("events.json"), EVENTS).unwrap();
    let entry = auto_trashed(&conversations, "17941219994", &[
        ("metadata.json", "{}"),
        ("events.json", "[]"),
    ]);

    let roots = restore_auto_trashed(&backend);

    assert!(roots[0].restored.is_empty());
    assert!(matches!(
        roots[0].left[0].reason,
        LeftReason::ConversationExists
    ));
    assert_eq!(
        fs::read_to_string(live.join("metadata.json")).unwrap(),
        r#"{"title":"a title"}"#
    );
    assert!(entry.join("metadata.json").is_file());
    assert!(!conversations.join("17941219994").exists());
}

// Only what startup validation moved is JP's to move back.
#[test]
fn leaves_entries_jp_did_not_trash_alone() {
    let (_tmp, backend, conversations) = setup();
    let unmarked = conversations.join(".trash").join("17941219995");
    fs::create_dir_all(&unmarked).unwrap();
    fs::write(unmarked.join("metadata.json"), "{}").unwrap();
    fs::write(unmarked.join("events.json"), "[]").unwrap();
    let other_note = conversations.join(".trash").join("17941219996");
    fs::create_dir_all(&other_note).unwrap();
    fs::write(other_note.join("TRASHED.md"), "Trashed by hand.").unwrap();

    let roots = restore_auto_trashed(&backend);

    assert!(roots[0].restored.is_empty());
    assert!(roots[0].left.is_empty());
    assert!(unmarked.join("metadata.json").is_file());
    assert!(other_note.join("TRASHED.md").is_file());
}

// A conversation the user trashed stays trashed, even when the restore has not
// yet run in that root.
#[test]
fn leaves_conversations_the_user_trashed_alone() {
    let (_tmp, backend, conversations) = setup();
    let live = conversations.join("17941219995");
    fs::create_dir_all(&live).unwrap();
    fs::write(live.join("events.json"), EVENTS).unwrap();
    backend
        .trash_conversation(&id("17941219995"), "2026-11-19T12:00:00Z".parse().unwrap())
        .unwrap();

    let roots = restore_auto_trashed(&backend);

    assert!(roots[0].restored.is_empty());
    assert!(roots[0].left.is_empty());
    assert!(
        conversations
            .join(".trash/17941219995/events.json")
            .is_file()
    );
    assert!(!live.exists());
}

// What was left is reported once; a later run does not look at the trash again.
#[test]
fn runs_once_per_root() {
    let (_tmp, backend, conversations) = setup();
    auto_trashed(&conversations, "17941219991", &[("events.json", EVENTS)]);
    assert_eq!(restored_ids(&restore_auto_trashed(&backend)), [id(
        "17941219991"
    )]);

    let later = auto_trashed(&conversations, "17941219992", &[("events.json", EVENTS)]);

    assert!(restore_auto_trashed(&backend).is_empty());
    assert!(later.join("events.json").is_file());
}

#[test]
fn a_root_without_a_trash_is_skipped() {
    let (_tmp, backend, conversations) = setup();

    assert!(restore_auto_trashed(&backend).is_empty());
    assert!(!conversations.join(".trash").exists());
}

// Another process holding the lock may be about to create the conversation, so
// the entry waits, and the next run tries it again.
#[test]
fn a_locked_conversation_is_retried_on_a_later_run() {
    let (_tmp, backend, conversations) = setup();
    let entry = auto_trashed(&conversations, "17941219991", &[("events.json", EVENTS)]);

    let lock = backend
        .try_lock(&id("17941219991").to_string(), None)
        .unwrap()
        .unwrap();
    let roots = restore_auto_trashed(&backend);
    assert!(roots[0].restored.is_empty());
    assert!(roots[0].left.is_empty());
    assert!(entry.join("events.json").is_file());

    drop(lock);
    assert_eq!(restored_ids(&restore_auto_trashed(&backend)), [id(
        "17941219991"
    )]);
}

// User-local storage has a trash of its own, restored the same way.
#[test]
fn restores_the_trash_of_every_storage_root() {
    let tmp = tempdir().unwrap();
    let backend = FsStorageBackend::new(&tmp.path().join(".jp"))
        .unwrap()
        .with_user_storage(&tmp.path().join("user"), None, "abc")
        .unwrap();
    let [workspace, user] = <[Utf8PathBuf; 2]>::try_from(backend.conversations_dirs()).unwrap();
    fs::create_dir_all(&workspace).unwrap();
    auto_trashed(&workspace, "17941219991", &[("events.json", EVENTS)]);
    auto_trashed(&user, "17941219992", &[("events.json", EVENTS)]);

    let roots = restore_auto_trashed(&backend);

    assert_eq!(restored_ids(&roots), [id("17941219991"), id("17941219992")]);
    assert!(user.join("17941219992/events.json").is_file());
}

fn root(left: Vec<Left>, restored: &[&str]) -> RootRestore {
    RootRestore {
        trash: Utf8PathBuf::from("/ws/.jp/conversations/.trash"),
        restored: restored
            .iter()
            .map(|dirname| Restored {
                id: id(dirname),
                dirname: (*dirname).to_owned(),
            })
            .collect(),
        left,
    }
}

fn left(dirname: &str, reason: LeftReason) -> Left {
    Left {
        id: ConversationId::try_from_dirname(dirname).ok(),
        dirname: dirname.to_owned(),
        reason,
    }
}

fn missing_events() -> LeftReason {
    LeftReason::Invalid(ValidationError::MissingFile {
        file: ManagedFile::Events,
    })
}

/// A report as the user reads it, with the styling stripped.
fn plain(roots: &[RootRestore]) -> Option<String> {
    render_text(roots).map(strip_ansi_escapes::strip_str)
}

#[test]
fn nothing_to_say_renders_nothing() {
    assert_eq!(render_text(&[root(vec![], &[])]), None);
    assert_eq!(
        render_json(&[root(vec![], &[])], false).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn restored_conversations_are_counted() {
    assert_eq!(
        plain(&[root(vec![], &["17941219991"])]).as_deref(),
        Some("Restored 1 conversation JP had moved to the trash.")
    );
    assert_eq!(
        plain(&[root(vec![], &["17941219991", "17941219992"])]).as_deref(),
        Some("Restored 2 conversations JP had moved to the trash.")
    );
}

#[test]
fn up_to_three_left_conversations_are_listed() {
    let report = plain(&[root(
        vec![
            left("17941219993", LeftReason::ConversationExists),
            left("17941219994", missing_events()),
            left(
                "not-an-id",
                LeftReason::Invalid(ValidationError::InvalidDirname),
            ),
        ],
        &["17941219991"],
    )]);

    assert_eq!(
        report.as_deref(),
        Some(
            "Restored 1 conversation JP had moved to the trash.\nLeft 3 conversations in \
             /ws/.jp/conversations/.trash:\n  jp-c17941219993: a conversation with this ID \
             exists\n  jp-c17941219994: missing events.json\n  not-an-id: invalid directory name"
        )
    );
}

#[test]
fn more_than_three_left_conversations_are_counted() {
    let report = plain(&[root(
        vec![
            left("17941219991", missing_events()),
            left("17941219992", missing_events()),
            left("17941219993", missing_events()),
            left("17941219994", missing_events()),
        ],
        &[],
    )]);

    assert_eq!(
        report.as_deref(),
        Some("Left 4 conversations in /ws/.jp/conversations/.trash. Run with `-v` to list them.")
    );
}

#[test]
fn the_counts_and_conversations_are_highlighted() {
    let report = render_text(&[root(vec![left("17941219994", missing_events())], &[
        "17941219991",
    ])])
    .unwrap();

    assert_eq!(
        report,
        format!(
            "Restored {} JP had moved to the trash.\nLeft {} in /ws/.jp/conversations/.trash:\n  \
             {}: missing events.json",
            "1 conversation".bold().yellow(),
            "1 conversation".bold().yellow(),
            "jp-c17941219994".bold().yellow(),
        )
    );
}

#[test]
fn json_is_one_record_per_trash() {
    let records = render_json(
        &[root(
            vec![
                left("17941219993", LeftReason::ConversationExists),
                left("17941219994", missing_events()),
            ],
            &["17941219991"],
        )],
        false,
    )
    .unwrap();

    assert_eq!(records, [concat!(
        r#"{"level":"info","code":"auto_trash_restored","#,
        r#""message":"restored conversations JP had moved to the trash","#,
        r#""context":{"trash":{"path":"/ws/.jp/conversations/.trash"}},"#,
        r#""restored":[{"id":"jp-c17941219991","dirname":"17941219991"}],"#,
        r#""left":[{"id":"jp-c17941219993","dirname":"17941219993","#,
        r#""path":"/ws/.jp/conversations/.trash/17941219993","#,
        r#""code":"conversation_exists","message":"a conversation with this ID exists"},"#,
        r#"{"id":"jp-c17941219994","dirname":"17941219994","#,
        r#""path":"/ws/.jp/conversations/.trash/17941219994","#,
        r#""code":"conversation_invalid","message":"conversation failed validation","#,
        r#""causes":[{"code":"file_missing","message":"missing file","#,
        r#""path":"/ws/.jp/conversations/.trash/17941219994/events.json"}]}]}"#,
    )]);
}
