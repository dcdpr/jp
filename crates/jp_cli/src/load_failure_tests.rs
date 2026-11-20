use std::{fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::{Chrome, OutputFormat};
use jp_storage::{
    backend::FsStorageBackend,
    validate::{FileError, ManagedFile, ValidationError},
};

use super::*;

fn failure<'a>(dirname: &'a str, error: &'a ValidationError) -> LoadFailure<'a> {
    LoadFailure {
        id: ConversationId::try_from_dirname(dirname).unwrap(),
        dirname,
        path: Utf8PathBuf::from(format!("/ws/.jp/conversations/{dirname}")),
        error,
    }
}

fn missing_metadata() -> ValidationError {
    ValidationError::MissingFile {
        file: ManagedFile::Metadata,
    }
}

fn corrupt_metadata() -> ValidationError {
    ValidationError::UnreadableFile {
        file: ManagedFile::Metadata,
        source: FileError::Json(serde_json::from_str::<serde_json::Value>("not json").unwrap_err()),
    }
}

/// A report as the user reads it, with the styling stripped.
fn plain(failures: &[LoadFailure<'_>]) -> Option<String> {
    render_text(failures).map(strip_ansi_escapes::strip_str)
}

#[test]
fn no_failures_renders_nothing() {
    assert_eq!(render_text(&[]), None);
    assert_eq!(render_json(&[], false).unwrap(), Vec::<String>::new());
}

#[test]
fn one_failure_is_listed_in_the_singular() {
    let error = missing_metadata();
    let report = plain(&[failure("17636257526", &error)]);

    assert_eq!(
        report.as_deref(),
        Some(
            "1 conversation failed to load:\n\n- jp-c17636257526: missing metadata.json\n\nRun \
             `jp conversation rm jp-c17636257526` to remove it."
        )
    );
}

// However many there are: the report is the only place a user learns of them.
#[test]
fn every_failure_is_listed() {
    let missing = missing_metadata();
    let corrupt = corrupt_metadata();
    let failures = [
        failure("17636257521", &missing),
        failure("17636257522", &corrupt),
        failure("17636257523", &missing),
        failure("17636257524", &missing),
        failure("17636257525", &missing),
        failure("17636257526", &missing),
    ];

    let expected = [
        "6 conversations failed to load:",
        "",
        "- jp-c17636257521: missing metadata.json",
        "- jp-c17636257522: metadata.json: invalid JSON at line 1, column 2",
        "- jp-c17636257523: missing metadata.json",
        "- jp-c17636257524: missing metadata.json",
        "- jp-c17636257525: missing metadata.json",
        "- jp-c17636257526: missing metadata.json",
        "",
        "Run `jp conversation rm jp-c17636257521 jp-c17636257522 jp-c17636257523 jp-c17636257524 \
         jp-c17636257525 jp-c17636257526` to remove them.",
    ];
    assert_eq!(plain(&failures), Some(expected.join("\n")));
}

// The count and each conversation are what the user acts on, so they carry the
// bold yellow JP uses to point at the thing a notice is about.
#[test]
fn the_count_and_conversations_are_highlighted() {
    let error = missing_metadata();
    let listed = render_text(&[failure("17636257526", &error)]).unwrap();
    assert_eq!(
        listed,
        format!(
            "{} failed to load:\n\n- {}: missing metadata.json\n\nRun `jp conversation rm \
             jp-c17636257526` to remove it.",
            "1 conversation".bold().yellow(),
            "jp-c17636257526".bold().yellow(),
        )
    );
}

#[test]
fn a_missing_file_is_one_record_with_one_cause() {
    let error = missing_metadata();
    let records = render_json(&[failure("17941219994", &error)], false).unwrap();

    assert_eq!(records, [concat!(
        r#"{"level":"warning","code":"conversation_load_failed","#,
        r#""message":"conversation failed to load","#,
        r#""context":{"conversation":{"id":"jp-c17941219994","dirname":"17941219994","#,
        r#""path":"/ws/.jp/conversations/17941219994"}},"#,
        r#""causes":[{"code":"file_missing","message":"missing file","#,
        r#""path":"/ws/.jp/conversations/17941219994/metadata.json"}]}"#,
    )]);
}

// The chain is split into one cause per step, so a script reads the file path
// and the parse position as fields instead of parsing them out of prose.
#[test]
fn a_corrupt_file_lists_the_file_and_the_parse_error_as_causes() {
    let error = corrupt_metadata();
    let records = render_json(&[failure("17941219994", &error)], false).unwrap();

    assert_eq!(records, [concat!(
        r#"{"level":"warning","code":"conversation_load_failed","#,
        r#""message":"conversation failed to load","#,
        r#""context":{"conversation":{"id":"jp-c17941219994","dirname":"17941219994","#,
        r#""path":"/ws/.jp/conversations/17941219994"}},"#,
        r#""causes":[{"code":"file_unreadable","message":"failed to load file","#,
        r#""path":"/ws/.jp/conversations/17941219994/metadata.json"},"#,
        r#"{"code":"json_invalid","message":"unable to parse json","#,
        r#""error":"expected ident at line 1 column 2","line":1,"column":2}]}"#,
    )]);
}

#[test]
fn an_unreadable_file_lists_the_io_error_as_a_cause() {
    let error = ValidationError::UnreadableFile {
        file: ManagedFile::Events,
        source: FileError::Io(io::Error::from(io::ErrorKind::PermissionDenied)),
    };
    let records = render_json(&[failure("17941219994", &error)], false).unwrap();

    assert_eq!(records, [concat!(
        r#"{"level":"warning","code":"conversation_load_failed","#,
        r#""message":"conversation failed to load","#,
        r#""context":{"conversation":{"id":"jp-c17941219994","dirname":"17941219994","#,
        r#""path":"/ws/.jp/conversations/17941219994"}},"#,
        r#""causes":[{"code":"file_unreadable","message":"failed to load file","#,
        r#""path":"/ws/.jp/conversations/17941219994/events.json"},"#,
        r#"{"code":"io_error","message":"unable to read file","error":"permission denied"}]}"#,
    )]);
}

/// Every conversation in a store holding one conversation whose `metadata.json`
/// is not JSON, as startup validation reports them.
fn validated_store(root: &Utf8Path) -> Vec<InvalidConversation> {
    let backend = FsStorageBackend::new(root).unwrap();
    backend
        .create_corrupt_test_conversation_dir("17941219994")
        .unwrap();
    backend.validate_conversations().invalid
}

// `--quiet` silences chrome, and a conversation JP cannot load is not chrome:
// the report is the only sign of it.
#[test]
fn the_text_report_prints_when_the_notice_drops_even_under_quiet() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let invalid = validated_store(tmp.path());
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_chrome(Chrome::Silenced);

    let report = notice(printer.clone(), &invalid);
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
         1, column 2\n\nRun `jp conversation rm jp-c17941219994` to remove it.\n"
    );
}

// The report prints after the command, so a conversation the command removed
// (`jp c rm`) or fixed is not reported as failing, nor offered for removal.
#[test]
fn a_conversation_the_command_dealt_with_is_not_reported() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let invalid = validated_store(tmp.path());
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);

    let report = notice(printer.clone(), &invalid);
    fs::remove_dir_all(tmp.path().join("conversations/17941219994")).unwrap();
    drop(report);
    printer.flush();

    assert_eq!(err.lock().as_str(), "");
}

#[test]
fn the_json_report_prints_raw_records_even_under_quiet() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let invalid = validated_store(tmp.path());
    let (printer, out, err) = Printer::memory(OutputFormat::Json);
    let printer = printer.with_chrome(Chrome::Silenced);

    drop(notice(printer, &invalid));

    assert_eq!(out.lock().as_str(), "");
    assert_eq!(
        err.lock().replace(tmp.path().as_str(), "[TMP]"),
        concat!(
            r#"{"level":"warning","code":"conversation_load_failed","#,
            r#""message":"conversation failed to load","#,
            r#""context":{"conversation":{"id":"jp-c17941219994","dirname":"17941219994","#,
            r#""path":"[TMP]/conversations/17941219994"}},"#,
            r#""causes":[{"code":"file_unreadable","message":"failed to load file","#,
            r#""path":"[TMP]/conversations/17941219994/metadata.json"},"#,
            r#"{"code":"json_invalid","message":"unable to parse json","#,
            r#""error":"expected ident at line 1 column 2","line":1,"column":2}]}"#,
            "\n",
        )
    );
}

#[test]
fn nothing_prints_when_every_conversation_loads() {
    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);

    drop(notice(printer.clone(), &[]));
    printer.flush();

    assert_eq!(out.lock().as_str(), "");
    assert_eq!(err.lock().as_str(), "");
}

// Scripts get every failure, never the human report's summary.
#[test]
fn json_lists_every_failure_past_three() {
    let error = missing_metadata();
    let records = render_json(
        &[
            failure("17636257521", &error),
            failure("17636257522", &error),
            failure("17636257523", &error),
            failure("17636257524", &error),
        ],
        false,
    )
    .unwrap();

    assert_eq!(records.len(), 4);
}
