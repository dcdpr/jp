use std::{fs, slice, sync::Arc};

use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::{Conversation, ConversationId};
use test_log::test;

use super::*;
use crate::{
    backend::{FsStorageBackend, LockBackend as _},
    validate::{ConversationPart, StorageLocation, ValidationError},
};

fn setup() -> (Utf8TempDir, Arc<FsStorageBackend>, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let storage = tmp.path().join("storage");
    let fs = Arc::new(FsStorageBackend::new(&storage).unwrap());
    let conversations = storage.join("conversations");
    (tmp, fs, conversations)
}

fn id(dirname: &str) -> ConversationId {
    ConversationId::try_from_dirname(dirname).unwrap()
}

fn broken(conversations: &Utf8PathBuf, dirname: &str) -> Utf8PathBuf {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("metadata.json"), "not json").unwrap();
    dir
}

fn scanned_paths(fs: &FsStorageBackend) -> Vec<Utf8PathBuf> {
    fs.scan()
        .unwrap()
        .iter()
        .map(InvalidConversation::path)
        .collect()
}

#[test]
fn a_store_of_valid_conversations_scans_clean() {
    let (_tmp, fs, _conversations) = setup();
    fs.write_test_conversation(&id("17041536000"), &Conversation::default());

    assert!(fs.scan().unwrap().is_empty());
}

// JP reports a broken conversation and leaves its files where they are: moving
// a user's data is the user's decision.
#[test]
fn invalid_conversations_are_reported_and_left_in_place() {
    let (_tmp, fs, conversations) = setup();
    fs.write_test_conversation(&id("17040672000"), &Conversation::default());
    let bad = broken(&conversations, "17041536000");

    // A directory whose name is not a conversation ID is not a conversation.
    let other = broken(&conversations, "not-a-valid-id");

    assert_eq!(scanned_paths(&fs), slice::from_ref(&bad));
    assert!(bad.join("metadata.json").is_file());
    assert!(other.join("metadata.json").is_file());
    assert!(!conversations.join(".trash").exists());
}

#[test]
fn dot_prefixed_directories_are_not_scanned() {
    let (_tmp, fs, conversations) = setup();
    broken(&conversations, ".trash");
    broken(&conversations, ".hidden");

    assert!(fs.scan().unwrap().is_empty());
}

#[test]
fn a_store_without_conversations_scans_clean() {
    let (_tmp, fs, _conversations) = setup();

    assert!(fs.scan().unwrap().is_empty());
}

// A conversation another process holds may be between the writes of its files,
// so its state says nothing yet.
#[test]
fn a_locked_conversation_is_not_reported_until_released() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17040672000");

    let lock = fs.try_lock("jp-c17040672000", None).unwrap().unwrap();
    assert!(fs.scan().unwrap().is_empty());

    drop(lock);
    assert_eq!(scanned_paths(&fs), [dir]);
}

#[test]
fn inspect_reports_why_a_conversation_does_not_load() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");

    let found = fs.inspect(&id("17941219991")).unwrap();

    assert_eq!(found.locations, [StorageLocation::new(dir.as_str())]);
    assert!(matches!(
        found.state,
        Err(ValidationError::Unreadable {
            part: ConversationPart::Metadata,
            ..
        })
    ));
}

#[test]
fn inspect_finds_nothing_for_an_id_without_a_directory() {
    let (_tmp, fs, _conversations) = setup();

    assert!(fs.inspect(&id("17941219991")).is_none());
}

#[test]
fn removal_deletes_every_directory_of_the_conversation() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991-a-title");

    let mut shown = vec![];
    let outcome = fs
        .remove(&id("17941219991"), &mut |found| {
            shown.clone_from(&found.locations);
            true
        })
        .unwrap();

    assert_eq!(outcome, Removal::Removed);
    assert_eq!(shown, [StorageLocation::new(dir.as_str())]);
    assert!(!dir.exists());
}

#[test]
fn a_declined_removal_keeps_the_conversation() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");

    let outcome = fs.remove(&id("17941219991"), &mut |_| false).unwrap();

    assert_eq!(outcome, Removal::Declined);
    assert!(dir.is_dir());
}

// The decision is never asked for while another process holds the
// conversation, so nothing it shows can change before the removal.
#[test]
fn a_conversation_another_process_holds_is_not_removed() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");
    let _held = fs.try_lock("jp-c17941219991", None).unwrap().unwrap();

    let outcome = fs
        .remove(&id("17941219991"), &mut |_| panic!("not asked"))
        .unwrap();

    assert_eq!(outcome, Removal::Locked);
    assert!(dir.is_dir());
}

#[test]
fn removing_an_id_without_a_directory_finds_nothing() {
    let (_tmp, fs, _conversations) = setup();

    let outcome = fs
        .remove(&id("17941219991"), &mut |_| panic!("not asked"))
        .unwrap();

    assert_eq!(outcome, Removal::NotFound);
}

// A dry run asks the same question and reports the same answer, and changes
// nothing on disk.
#[test]
fn a_dry_run_asks_and_removes_nothing() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");
    let dry = DryRunRecovery::new(fs);

    let mut asked = false;
    let outcome = dry
        .remove(&id("17941219991"), &mut |_| {
            asked = true;
            true
        })
        .unwrap();

    assert!(asked);
    assert_eq!(outcome, Removal::Removed);
    assert!(dir.is_dir());
    assert_eq!(dry.scan().unwrap().len(), 1);
}

#[test]
fn the_null_backend_holds_nothing() {
    let null = NullRecoveryBackend;

    assert!(null.scan().unwrap().is_empty());
    assert!(null.inspect(&id("17941219991")).is_none());
    assert_eq!(
        null.remove(&id("17941219991"), &mut |_| panic!("not asked"))
            .unwrap(),
        Removal::NotFound
    );
    assert!(null.diagnose().unwrap().invalid.is_empty());
    assert_eq!(
        null.repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| {
            panic!("not asked")
        })
        .unwrap(),
        RepairOutcome::NotFound
    );
}

/// A conversation directory holding valid events and nothing else.
fn events_only(conversations: &Utf8PathBuf, dirname: &str) -> Utf8PathBuf {
    let dir = conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("events.json"),
        r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request","content":"a"}]"#,
    )
    .unwrap();
    dir
}

// The diagnosis names the repair that makes a conversation load, and only where
// one does.
#[test]
fn the_diagnosis_offers_rebuilding_metadata_where_the_events_survive() {
    let (_tmp, fs, conversations) = setup();
    let rebuildable = events_only(&conversations, "17941219991-fix-the-loader");
    let corrupt = broken(&conversations, "17941219992");

    let diagnosis = fs.diagnose().unwrap();

    let found: Vec<_> = diagnosis
        .invalid
        .iter()
        .map(|diagnosed| (diagnosed.conversation.path(), diagnosed.repair))
        .collect();
    assert_eq!(found, vec![
        (rebuildable, Some(Repair::RebuildMetadata)),
        (corrupt, None),
    ]);
}

// Each stray entry is named for the directory in the store a user finds it
// under: a leftover file by the conversation directory it is in.
#[test]
fn the_diagnosis_lists_what_the_store_holds_beside_its_conversations() {
    let (_tmp, fs, conversations) = setup();
    fs.write_test_conversation(&id("17941219991"), &Conversation::default());
    let leftover = conversations.join("17941219991").join("events.json.tmp");
    fs::write(&leftover, "partial").unwrap();
    let notes = conversations.join("notes");
    fs::create_dir_all(&notes).unwrap();
    let staging_name = crate::import_staging_dirname("17941219992");
    let staging = conversations.join(&staging_name);
    fs::create_dir_all(&staging).unwrap();

    let diagnosis = fs.diagnose().unwrap();

    let strays: Vec<_> = diagnosis
        .strays
        .iter()
        .map(|stray| (stray.name.as_str(), stray.kind, stray.location.as_str()))
        .collect();
    assert!(diagnosis.invalid.is_empty());
    assert_eq!(strays, vec![
        (
            staging_name.as_str(),
            StrayKind::ImportStaging,
            staging.as_str()
        ),
        ("17941219991", StrayKind::TempFile, leftover.as_str()),
        ("notes", StrayKind::NotAConversation, notes.as_str()),
    ]);
    assert!(leftover.is_file());
}

// The startup scan never reads the archive, so the diagnosis is the only place
// an archived conversation that stopped loading shows up. A repair applies to
// live conversations only.
#[test]
fn the_diagnosis_covers_the_archive() {
    let (_tmp, fs, conversations) = setup();
    let archive = conversations.join(".archive");
    let live = broken(&conversations, "17941219991");
    let archived = events_only(&archive, "17941219992-old-work");
    let loads = events_only(&archive, "17941219993");
    fs::write(loads.join("metadata.json"), "{}").unwrap();

    let diagnosis = fs.diagnose().unwrap();

    let found: Vec<_> = diagnosis
        .invalid
        .iter()
        .map(|diagnosed| {
            (
                diagnosed.conversation.path(),
                diagnosed.conversation.archived,
                diagnosed.repair,
            )
        })
        .collect();
    assert_eq!(found, vec![
        (archived, true, None),
        (live.clone(), false, None)
    ]);
    assert_eq!(scanned_paths(&fs), [live]);
}

// A repair rebuilds live copies, so it is never offered for an archived copy,
// even of a conversation whose live copy it does fix.
#[test]
fn an_archived_copy_is_never_offered_the_live_copy_s_repair() {
    let (_tmp, fs, conversations) = setup();
    let live = events_only(&conversations, "17941219991");
    let archived = events_only(&conversations.join(".archive"), "17941219991");

    let diagnosis = fs.diagnose().unwrap();

    let found: Vec<_> = diagnosis
        .invalid
        .iter()
        .map(|diagnosed| (diagnosed.conversation.path(), diagnosed.repair))
        .collect();
    assert_eq!(found, vec![
        (archived, None),
        (live, Some(Repair::RebuildMetadata))
    ]);
}

#[test]
fn inspect_names_the_repair_that_makes_a_conversation_load() {
    let (_tmp, fs, conversations) = setup();
    events_only(&conversations, "17941219991");
    broken(&conversations, "17941219992");

    assert_eq!(
        fs.inspect(&id("17941219991")).unwrap().repair,
        Some(Repair::RebuildMetadata)
    );
    assert_eq!(fs.inspect(&id("17941219992")).unwrap().repair, None);
}

#[test]
fn rebuilding_metadata_makes_the_conversation_load() {
    let (_tmp, fs, conversations) = setup();
    events_only(&conversations, "17941219991-fix-the-loader");

    let outcome = fs
        .repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| true)
        .unwrap();

    assert_eq!(outcome, RepairOutcome::Repaired);
    assert!(fs.scan().unwrap().is_empty());
    assert_eq!(
        fs.inspect(&id("17941219991")).unwrap().title.as_deref(),
        Some("fix the loader")
    );
}

// A conversation with one copy that loads and one that does not has two copies
// that disagree: rebuilding the broken copy's metadata would make it load with
// metadata the other copy contradicts.
#[test]
fn metadata_is_not_rebuilt_beside_a_copy_that_loads() {
    let tmp = tempdir().unwrap();
    let fs = FsStorageBackend::new(&tmp.path().join("storage"))
        .unwrap()
        .with_user_storage(&tmp.path().join("user"), None, "abc")
        .unwrap();
    let workspace_copy = events_only(
        &tmp.path().join("storage").join("conversations"),
        "17941219991",
    );
    fs::write(workspace_copy.join("metadata.json"), "{}").unwrap();
    let user_copy = events_only(
        &tmp.path().join("user").join("abc").join("conversations"),
        "17941219991",
    );

    let diagnosis = fs.diagnose().unwrap();
    let outcome = fs
        .repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| {
            panic!("not asked")
        })
        .unwrap();

    let found: Vec<_> = diagnosis
        .invalid
        .iter()
        .map(|diagnosed| (diagnosed.conversation.path(), diagnosed.repair))
        .collect();
    assert_eq!(found, vec![(user_copy.clone(), None)]);
    assert_eq!(outcome, RepairOutcome::NotApplicable);
    assert!(!user_copy.join("metadata.json").exists());
}

// A repair that would not make the conversation load is refused before anyone
// is asked, and writes nothing.
#[test]
fn a_repair_that_does_not_apply_changes_nothing() {
    let (_tmp, fs, conversations) = setup();
    let dir = broken(&conversations, "17941219991");

    let outcome = fs
        .repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| {
            panic!("not asked")
        })
        .unwrap();

    assert_eq!(outcome, RepairOutcome::NotApplicable);
    assert_eq!(
        fs::read_to_string(dir.join("metadata.json")).unwrap(),
        "not json"
    );
}

#[test]
fn a_declined_repair_changes_nothing() {
    let (_tmp, fs, conversations) = setup();
    let dir = events_only(&conversations, "17941219991");

    let outcome = fs
        .repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| false)
        .unwrap();

    assert_eq!(outcome, RepairOutcome::Declined);
    assert!(!dir.join("metadata.json").exists());
}

#[test]
fn a_conversation_another_process_holds_is_not_repaired() {
    let (_tmp, fs, conversations) = setup();
    let dir = events_only(&conversations, "17941219991");
    let _held = fs.try_lock("jp-c17941219991", None).unwrap().unwrap();

    let outcome = fs
        .repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| {
            panic!("not asked")
        })
        .unwrap();

    assert_eq!(outcome, RepairOutcome::Locked);
    assert!(!dir.join("metadata.json").exists());
}

#[test]
fn a_dry_run_asks_and_repairs_nothing() {
    let (_tmp, fs, conversations) = setup();
    let dir = events_only(&conversations, "17941219991");
    let dry = DryRunRecovery::new(fs);

    let mut asked = false;
    let outcome = dry
        .repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| {
            asked = true;
            true
        })
        .unwrap();

    assert!(asked);
    assert_eq!(outcome, RepairOutcome::Repaired);
    assert!(!dir.join("metadata.json").exists());
}

// A conversation named for a repair that does not apply to it is reported as
// such, the same as the backend would, without anyone being asked.
#[test]
fn a_dry_run_reports_a_repair_that_does_not_apply() {
    let (_tmp, fs, conversations) = setup();
    broken(&conversations, "17941219991");
    let dry = DryRunRecovery::new(fs);

    let outcome = dry
        .repair(&id("17941219991"), Repair::RebuildMetadata, &mut |_| {
            panic!("not asked")
        })
        .unwrap();

    assert_eq!(outcome, RepairOutcome::NotApplicable);
}

#[test]
fn a_repair_is_named_by_its_stable_name() {
    assert_eq!(Repair::RebuildMetadata.name(), "rebuild-metadata");
    assert_eq!(
        Repair::from_name("rebuild-metadata"),
        Some(Repair::RebuildMetadata)
    );
    assert_eq!(Repair::from_name("rebuild"), None);
}
