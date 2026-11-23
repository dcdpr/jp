use std::{
    fs,
    sync::{Arc, Mutex},
};

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use jp_conversation::ConversationId;
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_storage::{
    Error as StorageError,
    backend::{
        ConversationLockGuard, DryRunRecovery, FsStorageBackend, LockBackend as _, RecoveryBackend,
        Removal, StoredConversation,
    },
    validate::{
        ConversationPart, InvalidConversation, Position, ReadError, StorageLocation,
        ValidationError,
    },
};

use super::*;
use crate::{cli::parse_args, load_failure::LoadFailure};

/// Where the filesystem backend keeps conversation `dirname`'s `file`.
fn file_location(dirname: &str, file: &str) -> StorageLocation {
    StorageLocation::labelled(format!("/ws/.jp/conversations/{dirname}/{file}"), file)
}

fn missing_metadata(dirname: &str) -> ValidationError {
    ValidationError::Missing {
        part: ConversationPart::Metadata,
        location: file_location(dirname, "metadata.json"),
    }
}

fn corrupt_events(dirname: &str) -> ValidationError {
    ValidationError::Unreadable {
        part: ConversationPart::Events,
        location: file_location(dirname, "events.json"),
        error: ReadError::Malformed {
            format: Some("JSON"),
            message: "expected value at line 1 column 2".to_owned(),
            position: Some(Position { line: 1, column: 2 }),
        },
    }
}

fn failure<'a>(dirname: &str, error: &'a ValidationError, fix: Option<Repair>) -> Finding<'a> {
    Finding {
        failure: LoadFailure {
            id: ConversationId::try_from_dirname(dirname).unwrap(),
            location: StorageLocation::new(format!("/ws/.jp/conversations/{dirname}")),
            error,
        },
        fix,
    }
}

/// A report as the user reads it, with the styling stripped.
fn plain(report: &Report<'_>) -> String {
    strip_ansi_escapes::strip_str(render_text(report))
}

#[test]
fn a_store_without_problems_reports_none() {
    let report = Report {
        failures: vec![],
        unrecognized: &[],
        leftovers: &[],
    };

    assert_eq!(plain(&report), "No problems found.");
    assert_eq!(render_json(&report, false).unwrap(), Vec::<String>::new());
}

// Each problem comes with what deals with it: the fix that makes a conversation
// load, the command that removes the rest, and `rm -r` for what is not a
// conversation.
#[test]
fn the_report_lists_each_problem_with_what_deals_with_it() {
    let missing = missing_metadata("17941219991");
    let corrupt = corrupt_events("17941219992");
    let unrecognized = [StorageLocation::new(
        "/Users/me/Library/Application Support/jp/conversations/notes",
    )];
    let leftovers = [
        StorageLocation::new("/ws/.jp/conversations/17941219993/events.json.tmp"),
        StorageLocation::new("/ws/.jp/conversations/17941219994/metadata.json.tmp"),
    ];
    let report = Report {
        failures: vec![
            failure("17941219991", &missing, Some(Repair::RebuildMetadata)),
            failure("17941219992", &corrupt, None),
        ],
        unrecognized: &unrecognized,
        leftovers: &leftovers,
    };

    assert_eq!(
        plain(&report),
        "\
Conversations

2 conversations fail to load:

- jp-c17941219991: missing metadata.json
- jp-c17941219992: events.json: invalid JSON at line 1, column 2

Run `jp doctor --fix rebuild-metadata` to make 1 of them load.
Run `jp conversation rm jp-c17941219992` to remove the rest.

1 directory in the conversation store is not a conversation:

- /Users/me/Library/Application Support/jp/conversations/notes

Run `rm -r '/Users/me/Library/Application Support/jp/conversations/notes'` to remove it.

2 leftovers of interrupted writes, which the next `jp` command removes:

- /ws/.jp/conversations/17941219993/events.json.tmp
- /ws/.jp/conversations/17941219994/metadata.json.tmp"
    );
}

#[test]
fn a_fix_that_covers_every_failure_is_the_only_suggestion() {
    let first = missing_metadata("17941219991");
    let second = missing_metadata("17941219992");
    let report = Report {
        failures: vec![
            failure("17941219991", &first, Some(Repair::RebuildMetadata)),
            failure("17941219992", &second, Some(Repair::RebuildMetadata)),
        ],
        unrecognized: &[],
        leftovers: &[],
    };

    assert_eq!(
        plain(&report),
        "\
Conversations

2 conversations fail to load:

- jp-c17941219991: missing metadata.json
- jp-c17941219992: missing metadata.json

Run `jp doctor --fix rebuild-metadata` to make them load."
    );
}

#[test]
fn without_a_fix_removal_is_the_only_suggestion() {
    let corrupt = corrupt_events("17941219992");
    let report = Report {
        failures: vec![failure("17941219992", &corrupt, None)],
        unrecognized: &[],
        leftovers: &[StorageLocation::new(
            "/ws/.jp/conversations/17941219993/events.json.tmp",
        )],
    };

    assert_eq!(
        plain(&report),
        "\
Conversations

1 conversation fails to load:

- jp-c17941219992: events.json: invalid JSON at line 1, column 2

Run `jp conversation rm jp-c17941219992` to remove it.

1 leftover of an interrupted write, which the next `jp` command removes:

- /ws/.jp/conversations/17941219993/events.json.tmp"
    );
}

// A conversation stored in both roots is one conversation: listed once, by the
// first copy's failure, and named once in the command that removes it.
#[test]
fn a_conversation_with_two_broken_copies_is_listed_once() {
    let corrupt = corrupt_events("17941219992");
    let missing = missing_metadata("17941219992");
    let mut user_copy = failure("17941219992", &missing, None);
    user_copy.failure.location = StorageLocation::new("/user/conversations/17941219992");
    let report = Report {
        failures: vec![failure("17941219992", &corrupt, None), user_copy],
        unrecognized: &[],
        leftovers: &[],
    };

    assert_eq!(
        plain(&report),
        "\
Conversations

1 conversation fails to load:

- jp-c17941219992: events.json: invalid JSON at line 1, column 2

Run `jp conversation rm jp-c17941219992` to remove it."
    );
}

#[test]
fn the_json_report_is_one_typed_record_per_problem() {
    let missing = missing_metadata("17941219991");
    let unrecognized = [StorageLocation::new("/ws/.jp/conversations/notes")];
    let leftovers = [StorageLocation::new(
        "/ws/.jp/conversations/17941219993/events.json.tmp",
    )];
    let report = Report {
        failures: vec![failure(
            "17941219991",
            &missing,
            Some(Repair::RebuildMetadata),
        )],
        unrecognized: &unrecognized,
        leftovers: &leftovers,
    };

    assert_eq!(render_json(&report, false).unwrap(), vec![
        r#"{"level":"warning","code":"conversation_load_failed","message":"conversation failed to load","context":{"conversation":{"id":"jp-c17941219991","location":"/ws/.jp/conversations/17941219991"}},"causes":[{"code":"part_missing","message":"missing conversation part","part":"metadata","location":"/ws/.jp/conversations/17941219991/metadata.json"}],"fix":"rebuild-metadata"}"#,
        r#"{"level":"warning","code":"not_a_conversation","message":"not a conversation","context":{"location":"/ws/.jp/conversations/notes"}}"#,
        r#"{"level":"info","code":"interrupted_write_leftover","message":"leftover of an interrupted write","context":{"location":"/ws/.jp/conversations/17941219993/events.json.tmp"}}"#,
    ]);
}

/// A store on disk, and a text printer capturing stdout.
struct Store {
    _tmp: Utf8TempDir,
    fs: Arc<FsStorageBackend>,
    conversations: Utf8PathBuf,
    printer: Printer,
    out: SharedBuffer,
}

fn store() -> Store {
    let tmp = camino_tempfile::tempdir().unwrap();
    let storage = tmp.path().join(".jp");
    let fs = Arc::new(FsStorageBackend::new(&storage).unwrap());
    let (printer, out, _err) = Printer::memory(OutputFormat::Text);

    Store {
        _tmp: tmp,
        fs,
        conversations: storage.join("conversations"),
        printer,
        out,
    }
}

/// A conversation directory holding valid events and nothing else.
fn events_only(store: &Store, dirname: &str) -> Utf8PathBuf {
    let dir = store.conversations.join(dirname);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("events.json"),
        r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request","content":"a"}]"#,
    )
    .unwrap();
    dir
}

fn doctor(args: &[&str]) -> Doctor {
    parse_args::<Doctor>(args).unwrap()
}

fn run(store: &Store, recovery: &dyn RecoveryBackend, args: &[&str], persists: bool) -> Output {
    let result = doctor(args).run(&store.printer, recovery, false, persists);
    store.printer.flush();
    result
}

// The report reads the store as it is: what the startup scan of any other
// command would remove stays where it is.
#[test]
fn reporting_changes_nothing_on_disk() {
    let s = store();
    let dir = events_only(&s, "17941219991");
    let leftover = dir.join("events.json.tmp");
    fs::write(&leftover, "partial").unwrap();

    run(&s, s.fs.as_ref(), &[], true).unwrap();

    assert!(leftover.is_file());
    assert!(!dir.join("metadata.json").exists());
}

#[test]
fn the_named_fix_makes_the_conversations_it_covers_load() {
    let s = store();
    let first = events_only(&s, "17941219991-fix-the-loader");
    let second = events_only(&s, "17941219992");

    run(
        &s,
        s.fs.as_ref(),
        &["--fix", "rebuild-metadata", "--no-confirm"],
        true,
    )
    .unwrap();

    assert_eq!(
        s.out.lock().as_str(),
        "Applied `rebuild-metadata` to 2 conversations.\n"
    );
    assert!(first.join("metadata.json").is_file());
    assert!(second.join("metadata.json").is_file());
    assert!(s.fs.scan().unwrap().is_empty());
}

// Writing to the user's conversations is never assumed when nobody can be asked.
#[test]
fn the_fix_needs_a_confirmation_when_nobody_can_answer() {
    let s = store();
    let dir = events_only(&s, "17941219991");

    let error = run(&s, s.fs.as_ref(), &["--fix", "rebuild-metadata"], true).unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some(
            "applying `rebuild-metadata` to 1 conversation needs a confirmation and nobody is \
             available to give one; pass --no-confirm to apply it without asking"
        )
    );
    assert!(!dir.join("metadata.json").exists());
}

#[test]
fn the_fix_under_no_persist_writes_nothing() {
    let s = store();
    let dir = events_only(&s, "17941219991");
    let dry = DryRunRecovery::new(Arc::clone(&s.fs) as _);

    run(
        &s,
        &dry,
        &["--fix", "rebuild-metadata", "--no-confirm"],
        false,
    )
    .unwrap();

    assert_eq!(
        s.out.lock().as_str(),
        "Applied `rebuild-metadata` to 1 conversation.\n\nThis command ran under `--no-persist`, \
         no files on disk were modified or removed.\n"
    );
    assert!(!dir.join("metadata.json").exists());
}

#[test]
fn a_fix_with_nothing_to_cover_changes_nothing() {
    let s = store();

    run(
        &s,
        s.fs.as_ref(),
        &["--fix", "rebuild-metadata", "--no-confirm"],
        true,
    )
    .unwrap();

    assert_eq!(
        s.out.lock().as_str(),
        "Nothing to fix with `rebuild-metadata`.\n"
    );
}

/// The store, with conversation `held`'s lock taken the moment the store has
/// been diagnosed, as by another process starting on that conversation between
/// the diagnosis and the fix.
///
/// A conversation already held when the store is diagnosed is left out of the
/// diagnosis, so only this order reaches a held conversation during the fix.
#[derive(Debug)]
struct LockAfterDiagnosis {
    fs: Arc<FsStorageBackend>,
    held: &'static str,
    guard: Mutex<Option<Box<dyn ConversationLockGuard>>>,
}

impl RecoveryBackend for LockAfterDiagnosis {
    fn scan(&self) -> Result<Vec<InvalidConversation>, StorageError> {
        self.fs.scan()
    }

    fn inspect(&self, id: &ConversationId) -> Option<StoredConversation> {
        self.fs.inspect(id)
    }

    fn remove(
        &self,
        id: &ConversationId,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<Removal, StorageError> {
        self.fs.remove(id, decide)
    }

    fn diagnose(&self) -> Result<Diagnosis, StorageError> {
        let diagnosis = self.fs.diagnose()?;
        *self.guard.lock().unwrap() = self.fs.try_lock(self.held, None)?;
        Ok(diagnosis)
    }

    fn repair(
        &self,
        id: &ConversationId,
        repair: Repair,
        decide: &mut dyn FnMut(&StoredConversation) -> bool,
    ) -> Result<RepairOutcome, StorageError> {
        self.fs.repair(id, repair, decide)
    }
}

// One conversation another process takes during the fix does not stop the
// rest: it is skipped, and named in the error the run ends with.
#[test]
fn the_fix_skips_and_reports_a_conversation_another_process_holds() {
    let s = store();
    let free = events_only(&s, "17941219991");
    let held = events_only(&s, "17941219992");
    let recovery = LockAfterDiagnosis {
        fs: Arc::clone(&s.fs),
        held: "jp-c17941219992",
        guard: Mutex::new(None),
    };

    let error = run(
        &s,
        &recovery,
        &["--fix", "rebuild-metadata", "--no-confirm"],
        true,
    )
    .unwrap_err();

    assert_eq!(
        s.out.lock().as_str(),
        "Applied `rebuild-metadata` to 1 conversation.\n"
    );
    assert_eq!(
        error.message.as_deref(),
        Some(
            "Failed to apply `rebuild-metadata` to 1 of 2 conversations:\n\njp-c17941219992:  in \
             use by another process"
        )
    );
    assert!(free.join("metadata.json").is_file());
    assert!(!held.join("metadata.json").exists());
}

// clap's message around the list of accepted values carries the usage line,
// which changes with every flag the command gains; the list is what this pins.
#[test]
fn an_unknown_fix_is_rejected_with_the_names_it_accepts() {
    let error = parse_args::<Doctor>(&["--fix", "rebuild"]).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("[possible values: rebuild-metadata]"),
        "{error}"
    );
}
