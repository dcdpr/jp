//! A one-time restore of the conversations older JP versions moved to the trash
//! on their own.
//!
//! Those versions moved every conversation startup validation could not load
//! into `conversations/.trash/`, with a `TRASHED.md` beside it.
//! JP no longer moves conversations by itself; [`restore_auto_trashed`] moves
//! those entries back, once per storage root, and [`notice`] reports what it
//! moved and what it left.
//!
//! Temporary: once users no longer carry auto-trashed conversations, this
//! module and its call in `run_inner` are deleted, and nothing else changes.

use std::{fmt, fs};

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_printer::Printer;
use jp_storage::{
    backend::{FsStorageBackend, LockBackend as _},
    validate::{ManagedFile, ValidationError, validate_conversation_dir},
};
use serde::Serialize;
use tracing::{debug, warn};

use crate::{
    deferred_report::DeferredReport,
    load_failure::{Cause, causes},
};

const TRASH_DIR: &str = ".trash";
const TRASHED_FILE: &str = "TRASHED.md";

/// The sentence in every `TRASHED.md` startup validation wrote.
///
/// An entry without it was not moved by JP, and is left alone.
const AUTO_TRASHED: &str = "failed workspace sanitization";

/// Written into a trash once its auto-trashed entries are dealt with.
const RESTORED_MARKER: &str = ".auto-trash-restored";

/// The most entries the text report lists one by one; above it, it gives a
/// count.
const LISTED_LEFT: usize = 3;

/// What the restore did in one storage root's trash.
#[derive(Debug)]
pub(crate) struct RootRestore {
    pub trash: Utf8PathBuf,
    pub restored: Vec<Restored>,
    pub left: Vec<Left>,
}

/// A conversation moved back out of the trash.
#[derive(Debug)]
pub(crate) struct Restored {
    pub id: ConversationId,
    pub dirname: String,
}

/// An auto-trashed entry the restore left in the trash.
#[derive(Debug)]
pub(crate) struct Left {
    /// `None` when the directory name is not a conversation ID.
    pub id: Option<ConversationId>,
    pub dirname: String,
    pub reason: LeftReason,
}

impl Left {
    /// The conversation ID, or the directory name when it is not one.
    fn name(&self) -> String {
        self.id
            .map_or_else(|| self.dirname.clone(), |id| id.to_string())
    }
}

#[derive(Debug)]
pub(crate) enum LeftReason {
    /// A live directory for the same conversation exists in some root.
    ConversationExists,

    /// The entry fails validation in a way moving it back would not help.
    Invalid(ValidationError),
}

impl fmt::Display for LeftReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConversationExists => f.write_str("a conversation with this ID exists"),
            Self::Invalid(error) => error.fmt(f),
        }
    }
}

/// What happened to one trash entry.
enum Outcome {
    Restored(Restored),
    Left(Left),
    /// Locked by another process, or the move failed: tried again next run.
    Retry,
}

/// Move auto-trashed conversations back, in every storage root whose trash has
/// not been restored yet.
///
/// Returns one entry per root it looked at.
pub(crate) fn restore_auto_trashed(fs: &FsStorageBackend) -> Vec<RootRestore> {
    fs.conversations_dirs()
        .iter()
        .filter_map(|conversations| restore_root(fs, conversations))
        .collect()
}

fn restore_root(fs: &FsStorageBackend, conversations: &Utf8Path) -> Option<RootRestore> {
    let trash = conversations.join(TRASH_DIR);
    if !trash.is_dir() || trash.join(RESTORED_MARKER).exists() {
        return None;
    }

    let mut root = RootRestore {
        trash,
        restored: vec![],
        left: vec![],
    };
    let mut retry = false;
    for dirname in auto_trashed_entries(&root.trash) {
        let source = root.trash.join(&dirname);
        match restore_entry(fs, conversations, &source, &dirname) {
            Outcome::Restored(restored) => root.restored.push(restored),
            Outcome::Left(left) => {
                warn!(
                    conversation = %left.name(),
                    path = %source,
                    reason = %left.reason,
                    "Left conversation in the trash."
                );
                root.left.push(left);
            }
            Outcome::Retry => retry = true,
        }
    }

    // Writing the marker only once nothing waits for a retry keeps a locked
    // entry from being stranded in the trash.
    if !retry
        && let Err(error) = fs::write(
            root.trash.join(RESTORED_MARKER),
            "JP restored the conversations it had moved here.\n",
        )
    {
        warn!(trash = %root.trash, %error, "Failed to mark the trash as restored.");
    }

    Some(root)
}

/// Names of the entries in `trash` that startup validation put there, sorted.
fn auto_trashed_entries(trash: &Utf8Path) -> Vec<String> {
    let Ok(entries) = trash.read_dir_utf8() else {
        return vec![];
    };

    let mut names: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir() && !entry.file_name().starts_with('.'))
        .filter(|entry| {
            fs::read_to_string(entry.path().join(TRASHED_FILE))
                .is_ok_and(|note| note.contains(AUTO_TRASHED))
        })
        .map(|entry| entry.file_name().to_owned())
        .collect();
    names.sort();
    names
}

fn restore_entry(
    fs: &FsStorageBackend,
    conversations: &Utf8Path,
    source: &Utf8Path,
    dirname: &str,
) -> Outcome {
    let left = |id, reason| {
        Outcome::Left(Left {
            id,
            dirname: dirname.to_owned(),
            reason,
        })
    };

    let Ok(id) = ConversationId::try_from_dirname(dirname) else {
        return left(None, LeftReason::Invalid(ValidationError::InvalidDirname));
    };
    if let Err(error) = validate_conversation_dir(source)
        && !restorable_without_metadata(source, &error)
    {
        return left(Some(id), LeftReason::Invalid(error));
    }

    // Held across the check and the move, so no other process creates the
    // conversation in between.
    let Ok(Some(_guard)) = fs.try_lock(&id.to_string(), None) else {
        debug!(%id, "Conversation is locked; restoring it on a later run.");
        return Outcome::Retry;
    };
    if fs.find_conversation_dir(&id).is_some() {
        return left(Some(id), LeftReason::ConversationExists);
    }

    let target = conversations.join(dirname);
    if let Err(error) = fs::rename(source, &target) {
        warn!(%id, %error, "Failed to restore conversation from the trash.");
        return Outcome::Retry;
    }
    if let Err(error) = fs::remove_file(target.join(TRASHED_FILE)) {
        warn!(%id, %error, "Failed to remove the restored conversation's TRASHED.md.");
    }

    debug!(%id, path = %target, "Restored conversation from the trash.");
    Outcome::Restored(Restored {
        id,
        dirname: dirname.to_owned(),
    })
}

/// Whether the entry has its events and only lacks `metadata.json`.
///
/// Moved back, it is reported as a load failure, which `jp conversation repair`
/// can rebuild the metadata for.
fn restorable_without_metadata(dir: &Utf8Path, error: &ValidationError) -> bool {
    matches!(error, ValidationError::MissingFile {
        file: ManagedFile::Metadata
    }) && dir.join(ManagedFile::Events.name()).is_file()
}

/// `1 conversation`, `2 conversations`.
fn conversations(count: usize) -> String {
    let noun = if count == 1 {
        "conversation"
    } else {
        "conversations"
    };
    format!("{count} {noun}")
}

/// Render the text report, or `None` when nothing was restored or left.
///
/// Up to three left entries per trash are listed with their name and reason;
/// more are summarized as a count.
pub(crate) fn render_text(roots: &[RootRestore]) -> Option<String> {
    let restored: usize = roots.iter().map(|root| root.restored.len()).sum();

    let mut lines = vec![];
    if restored > 0 {
        lines.push(format!(
            "Restored {} JP had moved to the trash.",
            conversations(restored).bold().yellow()
        ));
    }
    for root in roots.iter().filter(|root| !root.left.is_empty()) {
        lines.extend(render_left(root));
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn render_left(root: &RootRestore) -> Vec<String> {
    let subject = conversations(root.left.len()).bold().yellow();
    if root.left.len() > LISTED_LEFT {
        return vec![format!(
            "Left {subject} in {}. Run with `-v` to list them.",
            root.trash
        )];
    }

    let mut lines = vec![format!("Left {subject} in {}:", root.trash)];
    lines.extend(
        root.left
            .iter()
            .map(|left| format!("  {}: {}", left.name().bold().yellow(), left.reason)),
    );
    lines
}

/// Render one JSON record per trash that restored or left anything.
///
/// With `pretty`, each record is indented across several lines.
pub(crate) fn render_json(roots: &[RootRestore], pretty: bool) -> serde_json::Result<Vec<String>> {
    roots
        .iter()
        .filter(|root| !root.restored.is_empty() || !root.left.is_empty())
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

/// The report printed on stderr when it drops.
pub(crate) fn notice(printer: Printer, roots: &[RootRestore]) -> DeferredReport {
    DeferredReport::new(
        printer,
        || render_text(roots),
        |pretty| render_json(roots, pretty),
    )
}

/// One trash as a JSON record.
#[derive(Serialize)]
struct Record<'a> {
    level: &'static str,
    code: &'static str,
    message: &'static str,
    context: Context<'a>,
    restored: Vec<RestoredRecord<'a>>,
    left: Vec<LeftRecord<'a>>,
}

#[derive(Serialize)]
struct Context<'a> {
    trash: TrashContext<'a>,
}

#[derive(Serialize)]
struct TrashContext<'a> {
    path: &'a Utf8Path,
}

#[derive(Serialize)]
struct RestoredRecord<'a> {
    id: String,
    dirname: &'a str,
}

#[derive(Serialize)]
struct LeftRecord<'a> {
    /// `null` when the directory name is not a conversation ID.
    id: Option<String>,
    dirname: &'a str,
    path: Utf8PathBuf,
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    causes: Vec<Cause>,
}

impl<'a> From<&'a RootRestore> for Record<'a> {
    fn from(root: &'a RootRestore) -> Self {
        Self {
            level: "info",
            code: "auto_trash_restored",
            message: "restored conversations JP had moved to the trash",
            context: Context {
                trash: TrashContext { path: &root.trash },
            },
            restored: root
                .restored
                .iter()
                .map(|restored| RestoredRecord {
                    id: restored.id.to_string(),
                    dirname: &restored.dirname,
                })
                .collect(),
            left: root
                .left
                .iter()
                .map(|left| left_record(&root.trash, left))
                .collect(),
        }
    }
}

fn left_record<'a>(trash: &Utf8Path, left: &'a Left) -> LeftRecord<'a> {
    let path = trash.join(&left.dirname);
    let (code, message, causes) = match &left.reason {
        LeftReason::ConversationExists => (
            "conversation_exists",
            "a conversation with this ID exists",
            vec![],
        ),
        LeftReason::Invalid(error) => (
            "conversation_invalid",
            "conversation failed validation",
            causes(&path, error),
        ),
    };

    LeftRecord {
        id: left.id.map(|id| id.to_string()),
        dirname: &left.dirname,
        path,
        code,
        message,
        causes,
    }
}

#[cfg(test)]
#[path = "trash_restore_tests.rs"]
mod tests;
