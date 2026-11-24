//! Rebuilding a conversation's metadata from what its directory still holds.
//!
//! [`rebuilt_metadata`] decides the metadata from the [`Remains`] a directory
//! shows, and does no I/O.
//! [`rebuild_metadata`] reads those remains from disk and writes the result.

use std::fs;

use camino::Utf8Path;
use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId};

use crate::{
    EVENTS_FILE, METADATA_FILE, error::Result, validate::validate_events, value::write_json,
};

/// What a conversation directory without metadata still shows about the
/// conversation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Remains<'a> {
    pub(crate) id: ConversationId,

    /// The directory's name, which carries the title after the ID.
    pub(crate) dirname: &'a str,

    /// When the last event happened; `None` without events.
    pub(crate) last_event_at: Option<DateTime<Utc>>,

    /// When the directory was last modified.
    pub(crate) modified_at: Option<DateTime<Utc>>,

    /// Whether the directory is in the archive.
    pub(crate) archived: bool,
}

/// The metadata for the conversation `remains` describes.
///
/// The title is the one the directory name carries, with each dash read as a
/// space, since the name replaced the title's spaces and punctuation with
/// dashes.
/// The last activity is the time of the last event, or the conversation's
/// creation when it has none.
/// An archived conversation's archive time is the directory's modification
/// time, since nothing but the metadata records when it was archived.
/// Every other field takes its default.
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

/// Write the metadata of conversation `id`, stored in `dir`, from what the rest
/// of the directory holds; see [`rebuilt_metadata`].
///
/// `archived` says whether `dir` is in the archive.
/// Meant for a directory `missing_only_metadata` accepts: an existing
/// `metadata.json` is overwritten.
///
/// # Errors
///
/// Fails when the metadata cannot be written.
pub(crate) fn rebuild_metadata(dir: &Utf8Path, id: &ConversationId, archived: bool) -> Result<()> {
    // Read before the write below, which changes the directory's modification
    // time.
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

/// The title `dirname` carries after conversation `id`'s timestamp, with dashes
/// read as spaces.
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
