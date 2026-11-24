//! One-time restoration of conversations moved to `.trash/` by older JP
//! versions.
//!
//! Older versions moved conversation directories that failed startup validation
//! into `conversations/.trash/` alongside a `TRASHED.md` file.
//! [`restore_trash`] moves those directories back into `conversations/` so they
//! can be loaded or diagnosed by `jp doctor`, and [`notice`] reports any
//! entries that could not be moved.

use std::{collections::HashSet, fs, io, sync::Arc};

use camino::{Utf8DirEntry, Utf8Path, Utf8PathBuf};
use chrono::TimeDelta;
use crossterm::style::Stylize as _;
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_printer::Printer;
use jp_storage::{backend::FsStorageBackend, write_conversation_files};
use serde::Serialize;
use tracing::{debug, warn};

use crate::deferred_report::DeferredReport;

const TRASH_DIR: &str = ".trash";
const ARCHIVE_DIR: &str = ".archive";
const TRASHED_FILE: &str = "TRASHED.md";

/// Name of the store-wide lock held for the duration of [`restore_trash`].
const RESTORE_LOCK: &str = "trash-restore";

const CONVERSATION_FILES: [&str; 3] = ["metadata.json", "base_config.json", "events.json"];

/// A `.trash/` entry that [`restore_trash`] could not restore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Leftover {
    pub entry: Utf8PathBuf,
    pub reason: LeftoverReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LeftoverReason {
    /// A directory with the same non-ID name already exists in
    /// `conversations/`.
    NameTaken,

    /// An I/O or config error prevented restoring the entry.
    Failed(String),
}

impl LeftoverReason {
    const fn code(&self) -> &'static str {
        match self {
            Self::NameTaken => "name_taken",
            Self::Failed(_) => "io_error",
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::NameTaken => "a directory with this name already exists",
            Self::Failed(message) => message,
        }
    }
}

/// Restore every entry from each storage root's `.trash/` directory back into
/// `conversations/`, and remove `.trash/` once empty.
///
/// For each entry:
///
/// - Removes `TRASHED.md`.
/// - Deletes the directory if it is now empty.
/// - Initializes valid empty conversation files (using `config`, built lazily)
///   if the directory contains only a query draft.
/// - Allocates the next free [`ConversationId`] if the entry's ID is already in
///   use in the live or archive partition.
///
/// Holds a store-wide lock for the whole restore, blocking while another
/// process restores, so two processes never modify the same entry.
/// Reads `.trash/` only after acquiring the lock.
/// Skips the restore and logs a warning if the lock cannot be acquired.
///
/// Returns any entries that could not be restored.
pub(crate) fn restore_trash(
    fs: &FsStorageBackend,
    config: impl FnOnce() -> Option<Arc<AppConfig>>,
) -> Vec<Leftover> {
    let roots = fs.conversations_dirs();
    if !roots.iter().any(|root| root.join(TRASH_DIR).is_dir()) {
        return vec![];
    }

    let _restoring = match fs.lock_store(RESTORE_LOCK) {
        Ok(guard) => guard,
        Err(error) => {
            warn!(%error, "Not restoring the trash: failed to take its lock.");
            return vec![];
        }
    };

    let mut config = Some(config);
    let mut built = None;
    let mut base_config = || {
        if let Some(config) = config.take() {
            built = config();
        }
        built.clone()
    };

    let mut leftovers = vec![];
    for conversations in &roots {
        restore_root(&roots, conversations, &mut base_config, &mut leftovers);
    }
    leftovers
}

fn restore_root(
    roots: &[Utf8PathBuf],
    conversations: &Utf8Path,
    config: &mut dyn FnMut() -> Option<Arc<AppConfig>>,
    leftovers: &mut Vec<Leftover>,
) {
    let trash = conversations.join(TRASH_DIR);
    let Ok(entries) = trash.read_dir_utf8() else {
        return;
    };
    let mut entries: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(Utf8DirEntry::into_path)
        .collect();
    entries.sort();

    for entry in entries {
        if let Err(reason) = restore_entry(roots, conversations, &entry, config) {
            warn!(trash = %entry, reason = reason.message(), "Left entry in the trash.");
            leftovers.push(Leftover { entry, reason });
        }
    }

    // `remove_dir` succeeds only when `.trash/` is empty.
    if fs::remove_dir(&trash).is_ok() {
        debug!(%trash, "Removed the emptied trash.");
    }
}

fn restore_entry(
    roots: &[Utf8PathBuf],
    conversations: &Utf8Path,
    entry: &Utf8Path,
    config: &mut dyn FnMut() -> Option<Arc<AppConfig>>,
) -> Result<(), LeftoverReason> {
    let failed = |error: io::Error| LeftoverReason::Failed(error.to_string());

    match fs::remove_file(entry.join(TRASHED_FILE)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(error)),
    }

    // If removing `TRASHED.md` left the directory empty, delete it.
    if fs::remove_dir(entry).is_ok() {
        debug!(trash = %entry, "Removed an empty trash entry.");
        return Ok(());
    }

    let dirname = entry.file_name().unwrap_or_default();
    let Ok(id) = ConversationId::try_from_dirname(dirname) else {
        return move_entry(entry, &conversations.join(dirname));
    };

    if CONVERSATION_FILES
        .iter()
        .all(|file| !entry.join(file).exists())
    {
        let config = config().ok_or_else(|| {
            LeftoverReason::Failed("the workspace config did not load".to_owned())
        })?;
        let metadata = Conversation::default().with_last_activated_at(id.timestamp());
        let events = ConversationStream::new(config).with_created_at(id.timestamp());
        write_conversation_files(entry, &metadata, &events)
            .map_err(|error| LeftoverReason::Failed(error.to_string()))?;
    }

    let target = if ids_in(roots, &["", ARCHIVE_DIR]).contains(&id) {
        let free = next_free_id(id, &ids_in(roots, &["", ARCHIVE_DIR, TRASH_DIR]))
            .ok_or_else(|| LeftoverReason::Failed("no free conversation ID".to_owned()))?;
        conversations.join(free.to_dirname(None))
    } else {
        conversations.join(dirname)
    };
    move_entry(entry, &target)
}

fn move_entry(entry: &Utf8Path, target: &Utf8Path) -> Result<(), LeftoverReason> {
    if target.exists() {
        return Err(LeftoverReason::NameTaken);
    }

    fs::rename(entry, target).map_err(|error| LeftoverReason::Failed(error.to_string()))?;
    debug!(trash = %entry, path = %target, "Restored entry from the trash.");
    Ok(())
}

/// Collect all conversation IDs present in `partitions` (`""` for live) across
/// `roots`.
fn ids_in(roots: &[Utf8PathBuf], partitions: &[&str]) -> HashSet<ConversationId> {
    roots
        .iter()
        .flat_map(|root| partitions.iter().map(move |partition| root.join(partition)))
        .filter_map(|dir| dir.read_dir_utf8().ok())
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| ConversationId::try_from_dirname(entry.file_name()).ok())
        .collect()
}

/// Step size between consecutive [`ConversationId`] timestamps (1 decisecond).
const ID_STEP_MS: i64 = 100;

/// Return the first unused [`ConversationId`] after `id`, stepping by
/// [`ID_STEP_MS`].
fn next_free_id(id: ConversationId, used: &HashSet<ConversationId>) -> Option<ConversationId> {
    let mut candidate = id;
    loop {
        let next = candidate.timestamp() + TimeDelta::milliseconds(ID_STEP_MS);
        candidate = ConversationId::try_from(next).ok()?;
        if !used.contains(&candidate) {
            return Some(candidate);
        }
    }
}

pub(crate) fn notice(printer: Printer, leftovers: &[Leftover]) -> DeferredReport {
    DeferredReport::new(
        printer,
        || render_text(leftovers),
        |pretty| render_json(leftovers, pretty),
    )
}

pub(crate) fn render_text(leftovers: &[Leftover]) -> Option<String> {
    let (subject, pronoun) = match leftovers.len() {
        0 => return None,
        1 => ("1 entry".to_owned(), "it"),
        count => (format!("{count} entries"), "them"),
    };

    let entries = leftovers
        .iter()
        .map(|leftover| {
            format!(
                "- {}: {}",
                leftover.entry.as_str().bold().yellow(),
                leftover.reason.message()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    Some(format!(
        "{} could not be restored from the trash:\n\n{entries}\n\nMove or remove {pronoun} by \
         hand.",
        subject.bold().yellow()
    ))
}

pub(crate) fn render_json(leftovers: &[Leftover], pretty: bool) -> serde_json::Result<Vec<String>> {
    #[derive(Serialize)]
    struct Record<'a> {
        level: &'static str,
        code: &'static str,
        message: &'static str,
        context: Context<'a>,
        causes: [Cause<'a>; 1],
    }

    #[derive(Serialize)]
    struct Context<'a> {
        entry: Entry<'a>,
    }

    #[derive(Serialize)]
    struct Entry<'a> {
        location: &'a Utf8Path,
    }

    #[derive(Serialize)]
    struct Cause<'a> {
        code: &'static str,
        message: &'a str,
    }

    leftovers
        .iter()
        .map(|leftover| Record {
            level: "warning",
            code: "trash_entry_not_restored",
            message: "trash entry could not be restored",
            context: Context {
                entry: Entry {
                    location: &leftover.entry,
                },
            },
            causes: [Cause {
                code: leftover.reason.code(),
                message: leftover.reason.message(),
            }],
        })
        .map(|record| {
            if pretty {
                serde_json::to_string_pretty(&record)
            } else {
                serde_json::to_string(&record)
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "trash_restore_tests.rs"]
mod tests;
