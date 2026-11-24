//! Rebuilding `metadata.json` from the remaining state in a conversation
//! directory.
//!
//! [`rebuilt_metadata`] constructs the [`Conversation`] metadata from
//! [`Remains`] without performing I/O.
//! [`rebuild_metadata`] reads the directory state from disk and writes the
//! rebuilt `metadata.json`.

use std::fs;

use camino::Utf8Path;
use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId};

use crate::{
    EVENTS_FILE, METADATA_FILE, error::Result, validate::validate_events, value::write_json,
};

/// Information recoverable from a conversation directory that is missing
/// `metadata.json`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Remains<'a> {
    pub(crate) id: ConversationId,
    pub(crate) dirname: &'a str,
    pub(crate) last_event_at: Option<DateTime<Utc>>,
    pub(crate) modified_at: Option<DateTime<Utc>>,
    pub(crate) archived: bool,
}

/// Reconstruct [`Conversation`] metadata from `remains`.
///
/// - `title`: extracted from the directory slug after the conversation ID,
///   replacing dashes with spaces.
/// - `last_activated_at`: the timestamp of the last event, or the conversation
///   ID creation timestamp if there are no events.
/// - `archived_at`: the directory's modification time when `archived` is
///   `true`, or `None` for live conversations.
/// - All other fields use their [`Conversation::default`] values.
pub(crate) fn rebuilt_metadata(remains: &Remains<'_>) -> Conversation {
    Conversation {
        title: title_from_dirname(remains.dirname, &remains.id),
        last_activated_at: remains
            .last_event_at
            .unwrap_or_else(|| remains.id.timestamp()),
        archived_at: remains.modified_at.filter(|_| remains.archived),
        ..Conversation::default()
    }
}

/// Rebuild and write `metadata.json` for conversation `id` in `dir`; see
/// [`rebuilt_metadata`].
///
/// Overwrites any existing `metadata.json` in `dir`.
pub(crate) fn rebuild_metadata(dir: &Utf8Path, id: &ConversationId, archived: bool) -> Result<()> {
    // Capture the directory mtime before writing `metadata.json` updates it.
    let modified_at = fs::metadata(dir)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(DateTime::<Utc>::from);
    let last_event_at = validate_events(&dir.join(EVENTS_FILE))
        .ok()
        .and_then(|summary| summary.last_event_at);

    let remains = Remains {
        id: *id,
        dirname: dir.file_name().unwrap_or_default(),
        last_event_at,
        modified_at,
        archived,
    };
    write_json(&dir.join(METADATA_FILE), &rebuilt_metadata(&remains))
}

/// Extract the title slug after `id`'s timestamp prefix in `dirname`, replacing
/// dashes with spaces.
fn title_from_dirname(dirname: &str, id: &ConversationId) -> Option<String> {
    let slug = dirname
        .strip_prefix(&id.to_dirname(None))?
        .strip_prefix('-')?;

    let title = slug.replace('-', " ");
    (!title.trim().is_empty()).then_some(title)
}

#[cfg(test)]
#[path = "rebuild_tests.rs"]
mod tests;
