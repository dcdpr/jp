//! End-of-run reporting for conversations that fail validation.
//!
//! Startup validation leaves invalid conversations in place.
//! [`notice`] returns a [`LoadFailureNotice`] that re-checks each failed
//! conversation on drop (omitting any that the command repaired or removed) and
//! prints the remaining failures to stderr.

use std::sync::Arc;

use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::{
    backend::RecoveryBackend,
    validate::{InvalidConversation, ReadError, StorageLocation, ValidationError},
};
use serde::Serialize;
use tracing::warn;

use crate::deferred_report::DeferredReport;

/// A conversation that failed validation, formatted for end-of-run reporting.
#[derive(Debug)]
pub(crate) struct LoadFailure<'a> {
    pub id: ConversationId,
    pub location: StorageLocation,
    pub error: &'a ValidationError,
    pub archived: bool,
}

impl LoadFailure<'_> {
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

/// Log each failure and return a guard that prints the load-failure report to
/// stderr when dropped.
pub(crate) fn notice(
    printer: Printer,
    recovery: Arc<dyn RecoveryBackend>,
    failures: &[LoadFailure<'_>],
) -> LoadFailureNotice {
    for failure in failures {
        failure.log();
    }

    LoadFailureNotice {
        printer,
        recovery,
        failed: failures
            .iter()
            .map(|failure| Failed {
                id: failure.id,
                location: failure.location.clone(),
                archived: failure.archived,
            })
            .collect(),
    }
}

/// Guard that re-checks failed conversations on drop and prints any that still
/// fail to stderr.
pub(crate) struct LoadFailureNotice {
    printer: Printer,
    recovery: Arc<dyn RecoveryBackend>,
    failed: Vec<Failed>,
}

struct Failed {
    id: ConversationId,
    location: StorageLocation,
    archived: bool,
}

impl Drop for LoadFailureNotice {
    fn drop(&mut self) {
        let still_failing: Vec<_> = self
            .failed
            .drain(..)
            .filter_map(|failed| {
                let error = self.recovery.inspect(&failed.id)?.state.err()?;
                Some((failed, error))
            })
            .collect();

        let failures: Vec<_> = still_failing
            .iter()
            .map(|(failed, error)| LoadFailure {
                id: failed.id,
                location: failed.location.clone(),
                error,
                archived: failed.archived,
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
            archived: invalid.archived,
        }
    }
}

/// Maximum number of individual failures listed in the text report before
/// collapsing to a summary count.
const MAX_LISTED: usize = 5;

/// Render the text report, or `None` if `failures` is empty.
///
/// Lists up to [`MAX_LISTED`] failures with their ID and error, or a summary
/// count when more than [`MAX_LISTED`] conversations failed, followed by a hint
/// to run `jp doctor`.
pub(crate) fn render_text(failures: &[LoadFailure<'_>]) -> Option<String> {
    let count = failures.len();
    let noun = match count {
        0 => return None,
        1 => "conversation",
        _ => "conversations",
    };
    let archived = if failures.iter().all(|failure| failure.archived) {
        "archived "
    } else {
        ""
    };
    let subject = format!("{count} {archived}{noun}");

    if count > MAX_LISTED {
        return Some(format!(
            "{} failed to load. Run `jp doctor` to list them.",
            subject.bold().yellow()
        ));
    }

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

    let pronoun = if count == 1 { "it" } else { "them" };
    Some(format!(
        "{} failed to load:\n\n{entries}\n\nRun `jp doctor` to see how to fix {pronoun}.",
        subject.bold().yellow(),
    ))
}

/// Serialize one JSON record per failure.
pub(crate) fn render_json(
    failures: &[LoadFailure<'_>],
    pretty: bool,
) -> serde_json::Result<Vec<String>> {
    failures
        .iter()
        .map(|failure| render_record(failure, None, pretty))
        .collect()
}

/// Serialize a single failure as a JSON record, including `fix` when a `jp
/// doctor --fix` repair is available.
pub(crate) fn render_record(
    failure: &LoadFailure<'_>,
    fix: Option<&'static str>,
    pretty: bool,
) -> serde_json::Result<String> {
    let record = Record {
        fix,
        ..Record::from(failure)
    };

    if pretty {
        serde_json::to_string_pretty(&record)
    } else {
        serde_json::to_string(&record)
    }
}

/// JSON representation of a conversation load failure.
///
/// `code` fields are stable machine-readable identifiers; `message` fields are
/// human-readable descriptions.
#[derive(Serialize)]
struct Record<'a> {
    level: &'static str,
    code: &'static str,
    message: &'static str,
    context: Context<'a>,
    causes: Vec<Cause<'a>>,

    /// `jp doctor --fix` repair name, if an automatic repair is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<&'static str>,
}

#[derive(Serialize)]
struct Context<'a> {
    conversation: ConversationContext<'a>,
}

#[derive(Serialize)]
struct ConversationContext<'a> {
    id: String,
    location: &'a StorageLocation,

    #[serde(skip_serializing_if = "std::ops::Not::not")]
    archived: bool,
}

/// One entry in the validation error cause chain, outermost first.
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
            code: FAILURE_CODE,
            message: FAILURE_MESSAGE,
            context: Context {
                conversation: ConversationContext {
                    id: failure.id.to_string(),
                    location: &failure.location,
                    archived: failure.archived,
                },
            },
            causes: causes(failure.error),
            fix: None,
        }
    }
}

/// Serialize a [`ValidationError`] into the `{code, message, causes}` JSON
/// object used by `jp conversation show`.
pub(crate) fn failure_json(error: &ValidationError) -> serde_json::Value {
    #[derive(Serialize)]
    struct Failure<'a> {
        code: &'static str,
        message: &'static str,
        causes: Vec<Cause<'a>>,
    }

    serde_json::to_value(Failure {
        code: FAILURE_CODE,
        message: FAILURE_MESSAGE,
        causes: causes(error),
    })
    .unwrap_or_default()
}

const FAILURE_CODE: &str = "conversation_load_failed";
const FAILURE_MESSAGE: &str = "conversation failed to load";

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
        ReadError::Malformed {
            message, position, ..
        } => Cause {
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
