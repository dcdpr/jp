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
use crate::{cli::parse_args, load_failure::LoadFailure, shared::confirm::Answer};

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
            archived: false,
        },
        fix,
    }
}

fn archived_failure<'a>(
    dirname: &str,
    error: &'a ValidationError,
    fix: Option<Repair>,
) -> Finding<'a> {
    Finding {
        failure: LoadFailure {
            id: ConversationId::try_from_dirname(dirname).unwrap(),
            location: StorageLocation::new(format!("/ws/.jp/conversations/.archive/{dirname}")),
            error,
            archived: true,
        },
        fix,
    }
}

fn stray(name: &str, kind: StrayKind, location: &str) -> StrayEntry {
    StrayEntry {
        name: name.to_owned(),
        kind,
        location: StorageLocation::new(location),
    }
}

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

#[test]
fn archived_conversations_that_fail_to_load_have_their_own_section() {
    let missing = missing_metadata("17941219991");
    let archived_missing = missing_metadata("17941219993");
    let corrupt = corrupt_events("17941219992");
    let report = Report {
        failures: vec![
            failure("17941219991", &missing, Some(Repair::RebuildMetadata)),
            archived_failure("17941219992", &corrupt, None),
            archived_failure(
                "17941219993",
                &archived_missing,
                Some(Repair::RebuildMetadata),
            ),
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
- jp-c17941219992  events.json: invalid JSON at line 1, column 2    jp c rm jp-c17941219992
- jp-c17941219993  missing metadata.json                            jp doctor --id=jp-c17941219993 \
         --fix rebuild-metadata

Run `jp doctor --fix rebuild-metadata` to fix all 2 conversations it covers."
    );
}

#[test]
fn a_report_of_only_archived_problems_has_only_their_section() {
    let corrupt = corrupt_events("17941219992");
    let report = Report {
        failures: vec![archived_failure("17941219992", &corrupt, None)],
        strays: &[],
    };

    assert_eq!(
        plain(&report),
        "\
# Report
## Archived conversations
- jp-c17941219992  events.json: invalid JSON at line 1, column 2    jp c rm jp-c17941219992"
    );
}

#[test]
fn an_archived_conversation_s_record_says_it_is_archived() {
    let corrupt = corrupt_events("17941219992");
    let report = Report {
        failures: vec![archived_failure("17941219992", &corrupt, None)],
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

fn run_answering(
    store: &Store,
    args: &[&str],
    answers: &[Answer],
) -> (Output, Vec<(String, bool)>) {
    let mut answers = answers.iter().copied();
    let mut asked = vec![];
    let mut ask = |stored: &StoredConversation, more: bool| {
        asked.push((stored.id.to_string(), more));
        Ok(answers.next().expect("asked more often than answered"))
    };

    let result = doctor(args).run_with(&store.printer, store.fs.as_ref(), Some(&mut ask), true);
    store.printer.flush();
    (result, asked)
}

#[test]
fn a_clean_store_exits_successfully() {
    let s = store();

    run(&s, s.fs.as_ref(), &[], true).unwrap();

    assert_eq!(s.out.lock().as_str(), "No problems found.\n");
}

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
         line 1, column 2    jp c rm jp-c17941219991\n"
    );
}

#[test]
fn the_fix_repairs_an_archived_conversation_where_it_is() {
    let s = store();
    let archived = s.conversations.join(".archive").join("17941219991");
    fs::create_dir_all(&archived).unwrap();
    fs::write(
        archived.join("events.json"),
        r#"[{"timestamp":"2026-01-01T10:00:00Z","type":"chat_request","content":"a"}]"#,
    )
    .unwrap();

    run(
        &s,
        s.fs.as_ref(),
        &["--fix", "rebuild-metadata", "--yes"],
        true,
    )
    .unwrap();

    assert_eq!(
        s.out.lock().as_str(),
        "Applied `rebuild-metadata` to 1 conversation.\n"
    );
    assert!(archived.join("metadata.json").is_file());
    assert!(!s.conversations.join("17941219991").exists());
}

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
fn the_fix_asks_for_each_conversation() {
    let s = store();
    let declined = events_only(&s, "17941219991");
    let accepted = events_only(&s, "17941219992");

    let (result, asked) = run_answering(&s, &["--fix", "rebuild-metadata"], &[
        Answer::No,
        Answer::Yes,
    ]);

    result.unwrap();
    assert_eq!(asked, [
        ("jp-c17941219991".to_owned(), true),
        ("jp-c17941219992".to_owned(), false),
    ]);
    assert_eq!(
        s.out.lock().as_str(),
        "Applied `rebuild-metadata` to 1 conversation.\n"
    );
    assert!(!declined.join("metadata.json").exists());
    assert!(accepted.join("metadata.json").is_file());
}

#[test]
fn yes_to_all_applies_the_fix_to_every_remaining_conversation() {
    let s = store();
    let first = events_only(&s, "17941219991");
    let second = events_only(&s, "17941219992");

    let (result, asked) = run_answering(&s, &["--fix", "rebuild-metadata"], &[Answer::YesToAll]);

    result.unwrap();
    assert_eq!(asked, [("jp-c17941219991".to_owned(), true)]);
    assert_eq!(
        s.out.lock().as_str(),
        "Applied `rebuild-metadata` to 2 conversations.\n"
    );
    assert!(first.join("metadata.json").is_file());
    assert!(second.join("metadata.json").is_file());
}

#[test]
fn no_to_all_leaves_every_remaining_conversation_alone() {
    let s = store();
    let first = events_only(&s, "17941219991");
    let second = events_only(&s, "17941219992");

    let (result, asked) = run_answering(&s, &["--fix", "rebuild-metadata"], &[Answer::NoToAll]);

    result.unwrap();
    assert_eq!(asked, [("jp-c17941219991".to_owned(), true)]);
    assert_eq!(s.out.lock().as_str(), "Nothing changed.\n");
    assert!(!first.join("metadata.json").exists());
    assert!(!second.join("metadata.json").exists());
}

#[test]
fn one_named_conversation_is_asked_about_alone() {
    let s = store();
    let named = events_only(&s, "17941219991");
    events_only(&s, "17941219992");

    let (result, asked) = run_answering(
        &s,
        &["--fix", "rebuild-metadata", "--id=jp-c17941219991"],
        &[Answer::Yes],
    );

    result.unwrap();
    assert_eq!(asked, [("jp-c17941219991".to_owned(), false)]);
    assert!(named.join("metadata.json").is_file());
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

/// Test [`RecoveryBackend`] wrapper that locks `held` immediately after
/// [`Self::diagnose`] returns, simulating another process acquiring the
/// conversation between diagnosis and repair.
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

    fn scan_archive(&self) -> Result<Vec<InvalidConversation>, StorageError> {
        self.fs.scan_archive()
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
