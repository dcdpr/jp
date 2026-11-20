//! The report of conversations that failed to load.
//!
//! Startup validation finds conversation directories JP cannot load and leaves
//! them where they are.
//! [`notice`] prints the report on stderr after the command's own output:
//! [`render_text`] for the text formats, [`render_json`] for the JSON formats.
//! Each directory is checked again when the report prints, so one the command
//! removed or fixed is left out.

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::validate::{
    FileError, InvalidConversation, ValidationError, validate_conversation_dir,
};
use serde::Serialize;
use tracing::warn;

use crate::deferred_report::DeferredReport;

/// A conversation directory that failed to load, as the report shows it.
#[derive(Debug)]
pub(crate) struct LoadFailure<'a> {
    pub id: ConversationId,
    pub dirname: &'a str,
    pub path: Utf8PathBuf,
    pub error: &'a ValidationError,
}

impl LoadFailure<'_> {
    /// Log the failure as one warning, shown with `-v`.
    fn log(&self) {
        warn!(
            code = "conversation_load_failed",
            conversation = %self.id,
            path = %self.path,
            file = %self.path.join(self.error.file().name()),
            error = %self.error,
            "Conversation failed to load."
        );
    }
}

/// Log each failure, and return the report printed on stderr when it drops.
///
/// Under the JSON output formats the report is one record per failure.
pub(crate) fn notice(printer: Printer, invalid: &[InvalidConversation]) -> LoadFailureNotice {
    for failure in invalid.iter().map(LoadFailure::from) {
        failure.log();
    }

    LoadFailureNotice {
        printer,
        dirs: invalid
            .iter()
            .map(|invalid| (invalid.id, invalid.path()))
            .collect(),
    }
}

/// The load-failure report, printed on stderr when dropped.
///
/// Holds the conversation directories that failed startup validation, and
/// reports those that still exist and still fail when it drops.
pub(crate) struct LoadFailureNotice {
    printer: Printer,
    dirs: Vec<(ConversationId, Utf8PathBuf)>,
}

impl Drop for LoadFailureNotice {
    fn drop(&mut self) {
        let still_failing: Vec<_> = self
            .dirs
            .drain(..)
            .filter(|(_, dir)| dir.is_dir())
            .filter_map(|(id, dir)| {
                validate_conversation_dir(&dir)
                    .err()
                    .map(|error| (id, dir, error))
            })
            .collect();

        let failures: Vec<_> = still_failing
            .iter()
            .map(|(id, dir, error)| LoadFailure {
                id: *id,
                dirname: dir.file_name().unwrap_or_default(),
                path: dir.clone(),
                error,
            })
            .collect();

        drop(DeferredReport::new(
            self.printer.clone(),
            || render_text(&failures),
            |pretty| render_json(&failures, pretty),
        ));
    }
}

impl<'a> From<&'a InvalidConversation> for LoadFailure<'a> {
    fn from(invalid: &'a InvalidConversation) -> Self {
        Self {
            id: invalid.id,
            dirname: &invalid.dirname,
            path: invalid.path(),
            error: &invalid.error,
        }
    }
}

/// Render the text report, or `None` when nothing failed to load.
///
/// Every failure is listed with its ID and error, followed by the `jp
/// conversation rm` command that removes them.
pub(crate) fn render_text(failures: &[LoadFailure<'_>]) -> Option<String> {
    let count = failures.len();
    let noun = match count {
        0 => return None,
        1 => "conversation",
        _ => "conversations",
    };

    let entries = failures
        .iter()
        .map(|failure| {
            format!(
                "- {}: {}",
                failure.id.to_string().bold().yellow(),
                failure.error
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let subject = format!("{count} {noun}");

    Some(format!(
        "{} failed to load:\n\n{entries}\n\n{}",
        subject.bold().yellow(),
        rm_hint(failures)
    ))
}

/// The command that removes every listed failure.
fn rm_hint(failures: &[LoadFailure<'_>]) -> String {
    let ids: Vec<_> = failures
        .iter()
        .map(|failure| failure.id.to_string())
        .collect();
    let pronoun = if ids.len() == 1 { "it" } else { "them" };

    format!(
        "Run `jp conversation rm {}` to remove {pronoun}.",
        ids.join(" ")
    )
}

/// Render one JSON record per failure.
///
/// With `pretty`, each record is indented across several lines.
pub(crate) fn render_json(
    failures: &[LoadFailure<'_>],
    pretty: bool,
) -> serde_json::Result<Vec<String>> {
    failures
        .iter()
        .map(Record::from)
        .map(|record| {
            if pretty {
                serde_json::to_string_pretty(&record)
            } else {
                serde_json::to_string(&record)
            }
        })
        .collect()
}

/// One failure as a JSON record.
///
/// `code` fields are the typed part a script matches on; `message` fields are
/// prose that may be reworded.
#[derive(Serialize)]
struct Record<'a> {
    level: &'static str,
    code: &'static str,
    message: &'static str,
    context: Context<'a>,
    causes: Vec<Cause>,
}

#[derive(Serialize)]
struct Context<'a> {
    conversation: ConversationContext<'a>,
}

#[derive(Serialize)]
struct ConversationContext<'a> {
    id: String,
    dirname: &'a str,
    path: &'a Utf8Path,
}

/// One step of the error chain, outermost first.
#[derive(Serialize)]
struct Cause {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<Utf8PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<usize>,
}

impl Cause {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            path: None,
            error: None,
            line: None,
            column: None,
        }
    }
}

impl<'a> From<&'a LoadFailure<'a>> for Record<'a> {
    fn from(failure: &'a LoadFailure<'a>) -> Self {
        Self {
            level: "warning",
            code: "conversation_load_failed",
            message: "conversation failed to load",
            context: Context {
                conversation: ConversationContext {
                    id: failure.id.to_string(),
                    dirname: failure.dirname,
                    path: &failure.path,
                },
            },
            causes: causes(&failure.path, failure.error),
        }
    }
}

/// The error chain of the conversation directory at `dir`, outermost first.
fn causes(dir: &Utf8Path, error: &ValidationError) -> Vec<Cause> {
    match error {
        ValidationError::MissingFile { file } => vec![Cause {
            path: Some(dir.join(file.name())),
            ..Cause::new("file_missing", "missing file")
        }],
        ValidationError::UnreadableFile { file, source } => vec![
            Cause {
                path: Some(dir.join(file.name())),
                ..Cause::new("file_unreadable", "failed to load file")
            },
            file_cause(source),
        ],
    }
}

fn file_cause(source: &FileError) -> Cause {
    match source {
        FileError::Io(error) => Cause {
            error: Some(error.to_string()),
            ..Cause::new("io_error", "unable to read file")
        },
        FileError::Json(error) => Cause {
            error: Some(error.to_string()),
            line: Some(error.line()),
            column: Some(error.column()),
            ..Cause::new("json_invalid", "unable to parse json")
        },
    }
}

#[cfg(test)]
#[path = "load_failure_tests.rs"]
mod tests;
