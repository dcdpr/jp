//! The report of conversations that failed to load.
//!
//! Startup validation finds conversation directories JP cannot load and leaves
//! them where they are.
//! [`LoadFailureNotice`] prints the report on stderr after the command's own
//! output: [`render_text`] for the text formats, [`render_json`] for the JSON
//! formats.

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::validate::{FileError, InvalidConversation, ValidationError};
use serde::Serialize;
use tracing::warn;

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

/// Prints the load-failure report on stderr when dropped.
///
/// The report is printed past `--quiet`: conversations JP cannot load are never
/// left unsaid.
/// Under the JSON output formats it is one record per failure.
pub(crate) struct LoadFailureNotice {
    printer: Printer,
    lines: Vec<String>,
    /// Whether `lines` are JSON records, printed without the printer's
    /// `{"message": ...}` wrapping.
    raw: bool,
}

impl LoadFailureNotice {
    /// Log each failure and prepare the report for `printer`'s output format.
    pub(crate) fn new(printer: Printer, invalid: &[InvalidConversation]) -> Self {
        let failures: Vec<_> = invalid.iter().map(LoadFailure::from).collect();
        for failure in &failures {
            failure.log();
        }

        let format = printer.format();
        let json = format
            .is_json()
            .then(|| render_json(&failures, format.is_json_pretty()));

        let (lines, raw) = match json {
            Some(Ok(records)) => (records, true),
            Some(Err(error)) => {
                // The printer wraps text in a JSON record, so the stream stays
                // parseable either way.
                warn!(%error, "Failed to serialize the load-failure report.");
                (render_text(&failures).into_iter().collect(), false)
            }
            None => (render_text(&failures).into_iter().collect(), false),
        };

        Self {
            printer,
            lines,
            raw,
        }
    }
}

impl Drop for LoadFailureNotice {
    fn drop(&mut self) {
        if self.lines.is_empty() {
            return;
        }

        for line in self.lines.drain(..) {
            if self.raw {
                self.printer.error_println_raw(line);
            } else {
                self.printer.error_println(line);
            }
        }
        self.printer.flush();
    }
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
/// Up to three failures are listed with their name and error; more are
/// summarized as a count.
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
            Some(format!(
                "{} failed to load:\n{entries}",
                subject.bold().yellow()
            ))
        }
        _ => Some(format!(
            "{} {noun} failed to load. Run with `-v` to list them.",
            count.to_string().bold().yellow()
        )),
    }
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
                    id: failure.id.map(|id| id.to_string()),
                    dirname: failure.dirname,
                    path: &failure.path,
                },
            },
            causes: causes(failure),
        }
    }
}

fn causes(failure: &LoadFailure<'_>) -> Vec<Cause> {
    match failure.error {
        ValidationError::InvalidDirname => {
            vec![Cause::new("invalid_dirname", "invalid directory name")]
        }
        ValidationError::MissingFile { file } => vec![Cause {
            path: Some(failure.path.join(file.name())),
            ..Cause::new("file_missing", "missing file")
        }],
        ValidationError::UnreadableFile { file, source } => vec![
            Cause {
                path: Some(failure.path.join(file.name())),
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
