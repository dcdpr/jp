//! The report of conversations that failed to load.
//!
//! Startup validation finds conversation directories JP cannot load and leaves
//! them where they are.
//! [`notice`] prints the report on stderr after the command's own output:
//! [`render_text`] for the text formats, [`render_json`] for the JSON formats.

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::validate::{FileError, InvalidConversation, ValidationError};
use serde::Serialize;
use tracing::warn;

use crate::deferred_report::DeferredReport;

/// The most failures the text report lists one by one; above it, it gives a
/// count.
const LISTED_FAILURES: usize = 3;

/// A conversation directory that failed to load, as the report shows it.
#[derive(Debug)]
pub(crate) struct LoadFailure<'a> {
    /// `None` when the directory name is not a conversation ID.
    pub id: Option<ConversationId>,
    pub dirname: &'a str,
    pub path: Utf8PathBuf,
    pub error: &'a ValidationError,
}

impl LoadFailure<'_> {
    /// The conversation ID, or the directory name when it is not one.
    fn name(&self) -> String {
        self.id
            .map_or_else(|| self.dirname.to_owned(), |id| id.to_string())
    }

    /// Log the failure as one warning, shown with `-v`.
    fn log(&self) {
        let file = self.error.file().map(|file| self.path.join(file.name()));
        warn!(
            code = "conversation_load_failed",
            conversation = %self.name(),
            path = %self.path,
            file = file.as_deref().map(Utf8Path::as_str),
            error = %self.error,
            "Conversation failed to load."
        );
    }
}

/// Log each failure, and return the report printed on stderr when it drops.
///
/// Under the JSON output formats the report is one record per failure.
pub(crate) fn notice(printer: Printer, invalid: &[InvalidConversation]) -> DeferredReport {
    let failures: Vec<_> = invalid.iter().map(LoadFailure::from).collect();
    for failure in &failures {
        failure.log();
    }

    DeferredReport::new(
        printer,
        || render_text(&failures),
        |pretty| render_json(&failures, pretty),
    )
}

impl<'a> From<&'a InvalidConversation> for LoadFailure<'a> {
    fn from(invalid: &'a InvalidConversation) -> Self {
        Self {
            id: invalid.id(),
            dirname: &invalid.dirname,
            path: invalid.path(),
            error: &invalid.error,
        }
    }
}

/// Render the text report, or `None` when nothing failed to load.
///
/// Up to three failures are listed with their name and error, followed by the
/// `jp conversation trash` command that sets them aside; more are summarized as
/// a count.
pub(crate) fn render_text(failures: &[LoadFailure<'_>]) -> Option<String> {
    let count = failures.len();
    let noun = if count == 1 {
        "conversation"
    } else {
        "conversations"
    };

    match count {
        0 => None,
        1..=LISTED_FAILURES => {
            let entries = failures
                .iter()
                .map(|failure| format!("  {}: {}", failure.name().bold().yellow(), failure.error))
                .collect::<Vec<_>>()
                .join("\n");
            let subject = format!("{count} {noun}");
            let mut report = format!("{} failed to load:\n{entries}", subject.bold().yellow());
            if let Some(hint) = trash_hint(failures) {
                report.push('\n');
                report.push_str(&hint);
            }
            Some(report)
        }
        _ => Some(format!(
            "{} {noun} failed to load. Run with `-v` to list them.",
            count.to_string().bold().yellow()
        )),
    }
}

/// The command that trashes every listed failure with a conversation ID.
///
/// `None` when no failure has one: a directory whose name is not an ID cannot
/// be named to `jp conversation trash`.
fn trash_hint(failures: &[LoadFailure<'_>]) -> Option<String> {
    let ids: Vec<_> = failures
        .iter()
        .filter_map(|failure| failure.id)
        .map(|id| id.to_string())
        .collect();
    let pronoun = match ids.len() {
        0 => return None,
        1 => "it",
        _ => "them",
    };

    Some(format!(
        "Run `jp conversation trash {}` to move {pronoun} to the trash.",
        ids.join(" ")
    ))
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
    /// `null` when the directory name is not a conversation ID.
    id: Option<String>,
    dirname: &'a str,
    path: &'a Utf8Path,
}

/// One step of the error chain, outermost first.
#[derive(Serialize)]
pub(crate) struct Cause {
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
                    id: failure.id.map(|id| id.to_string()),
                    dirname: failure.dirname,
                    path: &failure.path,
                },
            },
            causes: causes(&failure.path, failure.error),
        }
    }
}

/// The error chain of the conversation directory at `dir`, outermost first.
pub(crate) fn causes(dir: &Utf8Path, error: &ValidationError) -> Vec<Cause> {
    match error {
        ValidationError::InvalidDirname => {
            vec![Cause::new("invalid_dirname", "invalid directory name")]
        }
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
