//! The report of conversations that failed to load.
//!
//! Startup validation finds conversation directories JP cannot load and leaves
//! them where they are.
//! [`notice`] prints the report on stderr after the command's own output:
//! [`render_text`] for the text formats, [`render_json`] for the JSON formats.
//! Each directory is checked again when the report prints, so one the command
//! removed or fixed is left out.

use camino::Utf8PathBuf;
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::validate::{
    InvalidConversation, ReadError, StorageLocation, ValidationError, validate_conversation_dir,
};
use serde::Serialize;
use tracing::warn;

use crate::deferred_report::DeferredReport;

/// A conversation directory that failed to load, as the report shows it.
#[derive(Debug)]
pub(crate) struct LoadFailure<'a> {
    pub id: ConversationId,
    pub location: StorageLocation,
    pub error: &'a ValidationError,
}

impl LoadFailure<'_> {
    /// Log the failure as one warning.
    fn log(&self) {
        warn!(
            code = "conversation_load_failed",
            conversation = %self.id,
            location = %self.location,
            part_location = %self.error.location(),
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
                location: StorageLocation::new(dir.as_str()),
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
            location: invalid.location(),
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
    causes: Vec<Cause<'a>>,
}

#[derive(Serialize)]
struct Context<'a> {
    conversation: ConversationContext<'a>,
}

#[derive(Serialize)]
struct ConversationContext<'a> {
    id: String,
    location: &'a StorageLocation,
}

/// One step of the error chain, outermost first.
#[derive(Serialize)]
struct Cause<'a> {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    part: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<&'a StorageLocation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<usize>,
}

impl Cause<'_> {
    const fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            part: None,
            location: None,
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
                    location: &failure.location,
                },
            },
            causes: causes(failure.error),
        }
    }
}

/// The error chain of a validation failure, outermost first.
fn causes(error: &ValidationError) -> Vec<Cause<'_>> {
    match error {
        ValidationError::Missing { part, location } => vec![Cause {
            part: Some(part.code()),
            location: Some(location),
            ..Cause::new("part_missing", "missing conversation part")
        }],
        ValidationError::Unreadable {
            part,
            location,
            error,
        } => vec![
            Cause {
                part: Some(part.code()),
                location: Some(location),
                ..Cause::new("part_unreadable", "failed to load conversation part")
            },
            read_cause(error),
        ],
    }
}

fn read_cause(error: &ReadError) -> Cause<'_> {
    match error {
        ReadError::Unavailable(message) => Cause {
            error: Some(message),
            ..Cause::new("unavailable", "unable to read")
        },
        ReadError::Malformed { message, position } => Cause {
            error: Some(message),
            line: position.map(|p| p.line),
            column: position.map(|p| p.column),
            ..Cause::new("malformed", "unable to parse")
        },
    }
}

#[cfg(test)]
#[path = "load_failure_tests.rs"]
mod tests;
