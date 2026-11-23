use std::{
    fs,
    sync::{Arc, Mutex},
};

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use clap::error::ErrorKind;
use crossterm::style::{Color, Stylize as _};
use jp_conversation::ConversationId;
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_storage::{
    Error as StorageError,
    backend::{
        ConversationLockGuard, DryRunRecovery, FsStorageBackend, LockBackend as _, RecoveryBackend,
        Removal, StoredConversation, StrayEntry, StrayKind,
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
        archived: false,
    }
}

fn archived_failure<'a>(dirname: &str, error: &'a ValidationError) -> Finding<'a> {
    Finding {
        failure: LoadFailure {
            id: ConversationId::try_from_dirname(dirname).unwrap(),
            location: StorageLocation::new(format!("/ws/.jp/conversations/.archive/{dirname}")),
            error,
        },
        fix: None,
        archived: true,
    }
}

fn stray(name: &str, kind: StrayKind, location: &str) -> StrayEntry {
    StrayEntry {
        name: name.to_owned(),
        kind,
        location: StorageLocation::new(location),
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
        strays: &[],
    };

    assert_eq!(plain(&report), "No problems found.");
    assert_eq!(render_json(&report, false).unwrap(), Vec::<String>::new());
}

// One row per problem: what it is about, what is wrong, and the command that
// deals with it.
#[test]
fn the_report_lists_each_problem_with_the_command_that_deals_with_it() {
    let missing = missing_metadata("17941219991");
    let corrupt = corrupt_events("17941219992");
    let strays = [
        stray(
            "notes",
            StrayKind::NotAConversation,
            "/Users/me/Library/Application Support/jp/conversations/notes",
        ),
        stray(
            "17941219993",
            StrayKind::TempFile,
            "/ws/.jp/conversations/17941219993/events.json.tmp",
        ),
    ];
    let report = Report {
        failures: vec![
            failure("17941219991", &missing, Some(Repair::RebuildMetadata)),
            failure("17941219992", &corrupt, None),
        ],
        strays: &strays,
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Conversations
- jp-c17941219991  missing metadata.json                            jp doctor --id=jp-c17941219991 \
         --fix rebuild-metadata
- jp-c17941219992  events.json: invalid JSON at line 1, column 2    jp c rm jp-c17941219992
- notes            directory is not a conversation                  rm -r \
         '/Users/me/Library/Application Support/jp/conversations/notes'
- 17941219993      unused temporary file                            rm -r \
         /ws/.jp/conversations/17941219993/events.json.tmp"
    );
}

// Each row's command fixes one conversation; a fix that covers more than one
// also gets the command that applies it to all of them.
#[test]
fn a_fix_covering_several_conversations_gets_one_command_for_all() {
    let first = missing_metadata("17941219991");
    let second = missing_metadata("17941219992");
    let report = Report {
        failures: vec![
            failure("17941219991", &first, Some(Repair::RebuildMetadata)),
            failure("17941219992", &second, Some(Repair::RebuildMetadata)),
        ],
        strays: &[],
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Conversations
- jp-c17941219991  missing metadata.json    jp doctor --id=jp-c17941219991 --fix rebuild-metadata
- jp-c17941219992  missing metadata.json    jp doctor --id=jp-c17941219992 --fix rebuild-metadata

Run `jp doctor --fix rebuild-metadata` to fix all 2 conversations it covers."
    );
}

// Two copies of one conversation are one conversation to fix.
#[test]
fn copies_of_one_conversation_count_once_toward_the_command_for_all() {
    let missing = missing_metadata("17941219991");
    let mut user_copy = failure("17941219991", &missing, Some(Repair::RebuildMetadata));
    user_copy.failure.location = StorageLocation::new("/user/conversations/17941219991");
    let report = Report {
        failures: vec![
            failure("17941219991", &missing, Some(Repair::RebuildMetadata)),
            user_copy,
        ],
        strays: &[],
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Conversations
- jp-c17941219991  missing metadata.json    jp doctor --id=jp-c17941219991 --fix rebuild-metadata"
    );
}

// An archived conversation is out of reach of the fixes and of `jp c rm`, so its
// row suggests unarchiving it, after which the report lists it with those.
#[test]
fn archived_conversations_that_fail_to_load_have_their_own_section() {
    let missing = missing_metadata("17941219991");
    let corrupt = corrupt_events("17941219992");
    let report = Report {
        failures: vec![
            failure("17941219991", &missing, Some(Repair::RebuildMetadata)),
            archived_failure("17941219992", &corrupt),
        ],
        strays: &[],
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Conversations
- jp-c17941219991  missing metadata.json    jp doctor --id=jp-c17941219991 --fix rebuild-metadata
## Archived conversations
- jp-c17941219992  events.json: invalid JSON at line 1, column 2    jp c unarchive jp-c17941219992"
    );
}

#[test]
fn a_report_of_only_archived_problems_has_only_their_section() {
    let corrupt = corrupt_events("17941219992");
    let report = Report {
        failures: vec![archived_failure("17941219992", &corrupt)],
        strays: &[],
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Archived conversations
- jp-c17941219992  events.json: invalid JSON at line 1, column 2    jp c unarchive jp-c17941219992"
    );
}

#[test]
fn an_archived_conversation_s_record_says_it_is_archived() {
    let corrupt = corrupt_events("17941219992");
    let report = Report {
        failures: vec![archived_failure("17941219992", &corrupt)],
        strays: &[],
    };

    assert_eq!(render_json(&report, false).unwrap(), vec![
        r#"{"level":"warning","code":"conversation_load_failed","message":"conversation failed to load","context":{"conversation":{"id":"jp-c17941219992","location":"/ws/.jp/conversations/.archive/17941219992","archived":true}},"causes":[{"code":"part_unreadable","message":"failed to load conversation part","part":"events","location":"/ws/.jp/conversations/17941219992/events.json"},{"code":"malformed","message":"unable to parse","error":"expected value at line 1 column 2","line":1,"column":2}]}"#,
    ]);
}

#[test]
fn an_unfinished_import_is_its_own_kind_of_problem() {
    let strays = [stray(
        ".import-17941219994-1",
        StrayKind::ImportStaging,
        "/ws/.jp/conversations/.import-17941219994-1",
    )];
    let report = Report {
        failures: vec![],
        strays: &strays,
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Conversations
- .import-17941219994-1  unused import directory    rm -r \
         /ws/.jp/conversations/.import-17941219994-1"
    );
}

// A conversation that does not load is red; what the store holds beside its
// conversations is orange.
#[test]
fn the_target_is_coloured_by_how_serious_the_problem_is() {
    let corrupt = corrupt_events("17941219992");
    let strays = [stray(
        "notes",
        StrayKind::NotAConversation,
        "/ws/.jp/conversations/notes",
    )];
    let report = Report {
        failures: vec![failure("17941219992", &corrupt, None)],
        strays: &strays,
    };

    assert_eq!(
        render_text(&report),
        format!(
            "# Report\n## Conversations\n- {}  events.json: invalid JSON at line 1, column 2    \
             jp c rm jp-c17941219992\n- {}            directory is not a conversation                  \
             rm -r /ws/.jp/conversations/notes",
            "jp-c17941219992".bold().red(),
            "notes".bold().with(Color::AnsiValue(208)),
        )
    );
}

// A conversation stored in both roots is one conversation: listed once, by the
// first copy's failure.
#[test]
fn a_conversation_with_two_broken_copies_is_listed_once() {
    let corrupt = corrupt_events("17941219992");
    let missing = missing_metadata("17941219992");
    let mut user_copy = failure("17941219992", &missing, None);
    user_copy.failure.location = StorageLocation::new("/user/conversations/17941219992");
    let report = Report {
        failures: vec![failure("17941219992", &corrupt, None), user_copy],
        strays: &[],
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Conversations
- jp-c17941219992  events.json: invalid JSON at line 1, column 2    jp c rm jp-c17941219992"
    );
}

#[test]
fn the_json_report_is_one_typed_record_per_problem() {
    let missing = missing_metadata("17941219991");
    let strays = [
        stray(
            "notes",
            StrayKind::NotAConversation,
            "/ws/.jp/conversations/notes",
        ),
        stray(
            "17941219993",
            StrayKind::TempFile,
            "/ws/.jp/conversations/17941219993/events.json.tmp",
        ),
    ];
    let report = Report {
        failures: vec![failure(
            "17941219991",
            &missing,
            Some(Repair::RebuildMetadata),
        )],
        strays: &strays,
    };

    assert_eq!(render_json(&report, false).unwrap(), vec![
        r#"{"level":"warning","code":"conversation_load_failed","message":"conversation failed to load","context":{"conversation":{"id":"jp-c17941219991","location":"/ws/.jp/conversations/17941219991"}},"causes":[{"code":"part_missing","message":"missing conversation part","part":"metadata","location":"/ws/.jp/conversations/17941219991/metadata.json"}],"fix":"rebuild-metadata"}"#,
        r#"{"level":"warning","code":"not_a_conversation","message":"directory is not a conversation","context":{"name":"notes","location":"/ws/.jp/conversations/notes"}}"#,
        r#"{"level":"warning","code":"unused_temporary_file","message":"unused temporary file","context":{"name":"17941219993","location":"/ws/.jp/conversations/17941219993/events.json.tmp"}}"#,
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

#[test]
fn a_clean_store_exits_successfully() {
    let s = store();

    run(&s, s.fs.as_ref(), &[], true).unwrap();

    assert_eq!(s.out.lock().as_str(), "No problems found.\n");
}

// Status 1 is a run that failed and 2 a mistyped command, so a script tells a
// report with problems from both. The report is the output; the status adds no
// message to it.
#[test]
fn a_report_with_problems_exits_with_status_3() {
    let s = store();
    events_only(&s, "17941219991");

    let error = run(&s, s.fs.as_ref(), &[], true).unwrap_err();

    assert_eq!(error.code.get(), 3);
    assert_eq!(error.message, None);
    assert!(error.expected);
    assert!(s.out.lock().starts_with("# Report\n"));
}

// The startup scan never reads the archive; the report does.
#[test]
fn an_archived_conversation_that_fails_to_load_is_reported() {
    let s = store();
    let archived = s.conversations.join(".archive").join("17941219991");
    fs::create_dir_all(&archived).unwrap();
    fs::write(archived.join("metadata.json"), "not json").unwrap();

    let error = run(&s, s.fs.as_ref(), &[], true).unwrap_err();

    assert_eq!(error.code.get(), 3);
    assert_eq!(
        s.out.lock().as_str(),
        "# Report\n## Archived conversations\n- jp-c17941219991  metadata.json: invalid JSON at \
         line 1, column 2    jp c unarchive jp-c17941219991\n"
    );
}

// The report reads the store as it is: what the startup scan of any other
// command would remove stays where it is.
#[test]
fn reporting_changes_nothing_on_disk() {
    let s = store();
    let dir = events_only(&s, "17941219991");
    let leftover = dir.join("events.json.tmp");
    fs::write(&leftover, "partial").unwrap();

    run(&s, s.fs.as_ref(), &[], true).unwrap_err();

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
        &["--fix", "rebuild-metadata", "--yes"],
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

#[test]
fn the_fix_applies_only_to_the_conversations_named() {
    let s = store();
    let named = events_only(&s, "17941219991");
    let other = events_only(&s, "17941219992");

    run(
        &s,
        s.fs.as_ref(),
        &["--fix", "rebuild-metadata", "--id=jp-c17941219991", "--yes"],
        true,
    )
    .unwrap();

    assert_eq!(
        s.out.lock().as_str(),
        "Applied `rebuild-metadata` to 1 conversation.\n"
    );
    assert!(named.join("metadata.json").is_file());
    assert!(!other.join("metadata.json").exists());
}

// A named conversation the fix cannot help is reported, not skipped silently.
#[test]
fn a_named_conversation_the_fix_cannot_help_is_reported() {
    let s = store();
    let broken = s.conversations.join("17941219991");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("metadata.json"), "not json").unwrap();

    let error = run(
        &s,
        s.fs.as_ref(),
        &[
            "--fix",
            "rebuild-metadata",
            "--id=jp-c17941219991",
            "--id=jp-c17941219992",
            "--yes",
        ],
        true,
    )
    .unwrap_err();

    assert_eq!(s.out.lock().as_str(), "");
    assert_eq!(
        error.message.as_deref(),
        Some(
            "Failed to apply `rebuild-metadata` to 2 of 2 conversations:\n\njp-c17941219991:  the \
             fix does not apply\njp-c17941219992:  no conversation with this ID"
        )
    );
    assert_eq!(
        fs::read_to_string(broken.join("metadata.json")).unwrap(),
        "not json"
    );
}

#[test]
fn naming_a_conversation_needs_a_fix() {
    let error = parse_args::<Doctor>(&["--id=jp-c17941219991"]).unwrap_err();

    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
}

// Writing to the user's conversations is never assumed when nobody can be asked.
#[test]
fn the_fix_needs_a_confirmation_when_nobody_can_answer() {
    let s = store();
    let dir = events_only(&s, "17941219991");

    let error = run(&s, s.fs.as_ref(), &["--fix", "rebuild-metadata"], true).unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some("Cannot run `--fix` in a non-interactive environment.")
    );
    assert!(!dir.join("metadata.json").exists());
}

#[test]
fn the_fix_under_no_persist_writes_nothing() {
    let s = store();
    let dir = events_only(&s, "17941219991");
    let dry = DryRunRecovery::new(Arc::clone(&s.fs) as _);

    run(&s, &dry, &["--fix", "rebuild-metadata", "--yes"], false).unwrap();

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
        &["--fix", "rebuild-metadata", "--yes"],
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

    let error = run(&s, &recovery, &["--fix", "rebuild-metadata", "--yes"], true).unwrap_err();

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
