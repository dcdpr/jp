use std::fs;

use camino::Utf8Path;
use jp_conversation::ConversationId;
use jp_printer::{Chrome, OutputFormat};
use jp_storage::{
    backend::{FsStorageBackend, NullRecoveryBackend},
    validate::{ConversationPart, Position, ReadError, StorageLocation, ValidationError},
};

use super::*;

fn failure<'a>(dirname: &str, error: &'a ValidationError) -> LoadFailure<'a> {
    LoadFailure {
        id: ConversationId::try_from_dirname(dirname).unwrap(),
        location: StorageLocation::new(format!("/ws/.jp/conversations/{dirname}")),
        error,
        archived: false,
    }
}

fn archived_failure<'a>(dirname: &str, error: &'a ValidationError) -> LoadFailure<'a> {
    LoadFailure {
        location: StorageLocation::new(format!("/ws/.jp/conversations/.archive/{dirname}")),
        archived: true,
        ..failure(dirname, error)
    }
}

fn file_location(file: &str) -> StorageLocation {
    StorageLocation::labelled(format!("/ws/.jp/conversations/17941219994/{file}"), file)
}

fn missing_metadata() -> ValidationError {
    ValidationError::Missing {
        part: ConversationPart::Metadata,
        location: file_location("metadata.json"),
    }
}

fn corrupt_metadata() -> ValidationError {
    ValidationError::Unreadable {
        part: ConversationPart::Metadata,
        location: file_location("metadata.json"),
        error: ReadError::Malformed {
            format: Some("JSON"),
            message: "expected ident at line 1 column 2".to_owned(),
            position: Some(Position { line: 1, column: 2 }),
        },
    }
}

fn plain(failures: &[LoadFailure<'_>]) -> Option<String> {
    render_text(failures).map(strip_ansi_escapes::strip_str)
}

#[test]
fn no_failures_renders_nothing() {
    assert_eq!(render_text(&[]), None);
    assert_eq!(render_json(&[], false).unwrap(), Vec::<String>::new());
}

#[test]
fn archived_failures_are_reported_as_archived() {
    let error = missing_metadata();

    assert_eq!(
        plain(&[archived_failure("17941219994", &error)]).as_deref(),
        Some(
            "1 archived conversation failed to load:\n\n- jp-c17941219994: missing \
             metadata.json\n\nRun `jp doctor` to see how to fix it."
        )
    );
}

#[test]
fn one_failure_is_listed_in_the_singular() {
    let error = missing_metadata();
    let report = plain(&[failure("17636257526", &error)]);

    assert_eq!(
        report.as_deref(),
        Some(
            "1 conversation failed to load:\n\n- jp-c17636257526: missing metadata.json\n\nRun \
             `jp doctor` to see how to fix it."
        )
    );
}

#[test]
fn up_to_five_failures_are_listed() {
    let missing = missing_metadata();
    let corrupt = corrupt_metadata();
    let failures = [
        failure("17636257521", &missing),
        failure("17636257522", &corrupt),
        failure("17636257523", &missing),
        failure("17636257524", &missing),
        failure("17636257525", &missing),
    ];

    let expected = [
        "5 conversations failed to load:",
        "",
        "- jp-c17636257521: missing metadata.json",
        "- jp-c17636257522: metadata.json: invalid JSON at line 1, column 2",
        "- jp-c17636257523: missing metadata.json",
        "- jp-c17636257524: missing metadata.json",
        "- jp-c17636257525: missing metadata.json",
        "",
        "Run `jp doctor` to see how to fix them.",
    ];
    assert_eq!(plain(&failures), Some(expected.join("\n")));
}

#[test]
fn more_than_five_failures_point_at_doctor() {
    let missing = missing_metadata();
    let failures = [
        failure("17636257521", &missing),
        failure("17636257522", &missing),
        failure("17636257523", &missing),
        failure("17636257524", &missing),
        failure("17636257525", &missing),
        failure("17636257526", &missing),
    ];

    assert_eq!(
        plain(&failures).as_deref(),
        Some("6 conversations failed to load. Run `jp doctor` to list them.")
    );
    assert_eq!(
        render_text(&failures).unwrap(),
        format!(
            "{} failed to load. Run `jp doctor` to list them.",
            "6 conversations".bold().yellow()
        )
    );
}

#[test]
fn the_count_and_conversations_are_highlighted() {
    let error = missing_metadata();
    let listed = render_text(&[failure("17636257526", &error)]).unwrap();
    assert_eq!(
        listed,
        format!(
            "{} failed to load:\n\n- {}: missing metadata.json\n\nRun `jp doctor` to see how to \
             fix it.",
            "1 conversation".bold().yellow(),
            "jp-c17636257526".bold().yellow(),
        )
    );
}

#[test]
fn a_missing_part_is_one_record_with_one_cause() {
    let error = missing_metadata();
    let records = render_json(&[failure("17941219994", &error)], false).unwrap();

    assert_eq!(records, [concat!(
        r#"{"level":"warning","code":"conversation_load_failed","#,
        r#""message":"conversation failed to load","#,
        r#""context":{"conversation":{"id":"jp-c17941219994","#,
        r#""location":"/ws/.jp/conversations/17941219994"}},"#,
        r#""causes":[{"code":"part_missing","message":"missing conversation part","#,
        r#""part":"metadata","location":"/ws/.jp/conversations/17941219994/metadata.json"}]}"#,
    )]);
}

#[test]
fn a_malformed_part_lists_the_part_and_the_parse_error_as_causes() {
    let error = corrupt_metadata();
    let records = render_json(&[failure("17941219994", &error)], false).unwrap();

    assert_eq!(records, [concat!(
        r#"{"level":"warning","code":"conversation_load_failed","#,
        r#""message":"conversation failed to load","#,
        r#""context":{"conversation":{"id":"jp-c17941219994","#,
        r#""location":"/ws/.jp/conversations/17941219994"}},"#,
        r#""causes":[{"code":"part_unreadable","message":"failed to load conversation part","#,
        r#""part":"metadata","location":"/ws/.jp/conversations/17941219994/metadata.json"},"#,
        r#"{"code":"malformed","message":"unable to parse","#,
        r#""error":"expected ident at line 1 column 2","line":1,"column":2}]}"#,
    )]);
}

#[test]
fn an_unavailable_part_lists_the_read_error_as_a_cause() {
    let error = ValidationError::Unreadable {
        part: ConversationPart::Events,
        location: file_location("events.json"),
        error: ReadError::Unavailable("permission denied".to_owned()),
    };
    let records = render_json(&[failure("17941219994", &error)], false).unwrap();

    assert_eq!(records, [concat!(
        r#"{"level":"warning","code":"conversation_load_failed","#,
        r#""message":"conversation failed to load","#,
        r#""context":{"conversation":{"id":"jp-c17941219994","#,
        r#""location":"/ws/.jp/conversations/17941219994"}},"#,
        r#""causes":[{"code":"part_unreadable","message":"failed to load conversation part","#,
        r#""part":"events","location":"/ws/.jp/conversations/17941219994/events.json"},"#,
        r#"{"code":"unavailable","message":"unable to read","error":"permission denied"}]}"#,
    )]);
}

fn failures(invalid: &[InvalidConversation]) -> Vec<LoadFailure<'_>> {
    invalid.iter().map(LoadFailure::from).collect()
}

fn scanned_store(root: &Utf8Path) -> (Arc<dyn RecoveryBackend>, Vec<InvalidConversation>) {
    let backend = Arc::new(FsStorageBackend::new(root).unwrap());
    backend
        .create_corrupt_test_conversation_dir("17941219994")
        .unwrap();
    let invalid = backend.scan().unwrap();
    (backend, invalid)
}

#[test]
fn the_text_report_prints_when_the_notice_drops_even_under_quiet() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let (recovery, invalid) = scanned_store(tmp.path());
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_chrome(Chrome::Silenced);

    let report = notice(printer.clone(), recovery, &failures(&invalid));
    printer.flush();
    assert_eq!(
        err.lock().as_str(),
        "",
        "nothing is printed before the drop"
    );

    drop(report);

    assert_eq!(out.lock().as_str(), "");
    assert_eq!(
        strip_ansi_escapes::strip_str(err.lock().as_str()),
        "1 conversation failed to load:\n\n- jp-c17941219994: metadata.json: invalid JSON at line \
         1, column 2\n\nRun `jp doctor` to see how to fix it.\n"
    );
}

#[test]
fn a_conversation_the_command_dealt_with_is_not_reported() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let (recovery, invalid) = scanned_store(tmp.path());
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);

    let report = notice(printer.clone(), recovery, &failures(&invalid));
    fs::remove_dir_all(tmp.path().join("conversations/17941219994")).unwrap();
    drop(report);
    printer.flush();

    assert_eq!(err.lock().as_str(), "");
}

#[test]
fn the_json_report_prints_raw_records_even_under_quiet() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let (recovery, invalid) = scanned_store(tmp.path());
    let (printer, out, err) = Printer::memory(OutputFormat::Json);
    let printer = printer.with_chrome(Chrome::Silenced);

    drop(notice(printer, recovery, &failures(&invalid)));

    // JSON escapes a Windows path's backslashes, so they become `/` before the
    // temporary directory is replaced.
    let tmp_slashed = tmp.path().as_str().replace('\\', "/");
    assert_eq!(out.lock().as_str(), "");
    assert_eq!(
        err.lock()
            .replace(r"\\", "/")
            .replace(&tmp_slashed, "[TMP]"),
        concat!(
            r#"{"level":"warning","code":"conversation_load_failed","#,
            r#""message":"conversation failed to load","#,
            r#""context":{"conversation":{"id":"jp-c17941219994","#,
            r#""location":"[TMP]/conversations/17941219994"}},"#,
            r#""causes":[{"code":"part_unreadable","message":"failed to load conversation part","#,
            r#""part":"metadata","location":"[TMP]/conversations/17941219994/metadata.json"},"#,
            r#"{"code":"malformed","message":"unable to parse","#,
            r#""error":"expected ident at line 1 column 2","line":1,"column":2}]}"#,
            "\n",
        )
    );
}

#[test]
fn nothing_prints_when_every_conversation_loads() {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);

    drop(notice(printer.clone(), Arc::new(NullRecoveryBackend), &[]));
    printer.flush();

    assert_eq!(out.lock().as_str(), "");
    assert_eq!(err.lock().as_str(), "");
}

#[test]
fn json_lists_every_failure_past_five() {
    let error = missing_metadata();
    let records = render_json(
        &[
            failure("17636257521", &error),
            failure("17636257522", &error),
            failure("17636257523", &error),
            failure("17636257524", &error),
            failure("17636257525", &error),
            failure("17636257526", &error),
        ],
        false,
    )
    .unwrap();

    assert_eq!(records.len(), 6);
}
