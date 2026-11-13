use std::{
    collections::{HashMap, HashSet},
    fs, io,
    time::SystemTime,
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_config::PartialAppConfig;
use jp_conversation::{Conversation, ConversationId, ConversationStream, StreamError};
use rayon::iter::{IntoParallelRefIterator as _, ParallelIterator as _};
use serde::de::DeserializeOwned;
use tracing::warn;

use crate::{
    ARCHIVE_DIR, BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE, Storage,
    backend::{ConversationFilter, ConversationIndexEntry, StoragePresence},
    build_conversation_dir_prefix, dir_entries, find_conversation_dir_path,
    load_conversation_id_from_entry,
    scan::{EventsSummary, FileStamp},
};

type Result<T> = std::result::Result<T, LoadError>;

#[derive(Debug, thiserror::Error)]
#[cfg_attr(test, derive(PartialEq))]
#[error("unable to load {}", .path)]
pub struct LoadError {
    path: Utf8PathBuf,

    #[source]
    error: LoadErrorInner,
}

impl LoadError {
    /// Create a new `LoadError` with the given path and inner error.
    pub fn new(path: impl Into<Utf8PathBuf>, error: LoadErrorInner) -> Self {
        Self {
            path: path.into(),
            error,
        }
    }

    /// Gets the underlying [`LoadErrorInner`] that provides more details on
    /// what went wrong.
    #[must_use]
    pub fn kind(&self) -> &LoadErrorInner {
        &self.error
    }

    /// Returns `true` if the error is caused by corrupt or invalid data (as
    /// opposed to a system-level I/O failure or missing data).
    #[must_use]
    pub fn is_corrupt(&self) -> bool {
        matches!(
            self.error,
            LoadErrorInner::Json(_) | LoadErrorInner::Stream(_)
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadErrorInner {
    #[error(transparent)]
    IO(#[from] io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("invalid conversation stream: {0}")]
    Stream(StreamError),

    #[error("conversation stream not found for {0}")]
    MissingConversationStream(ConversationId),

    #[error("conversation metadata not found for {0}")]
    MissingConversationMetadata(ConversationId),
}

impl LoadErrorInner {
    /// Returns whether the error is a missing conversation stream or metadata.
    #[must_use]
    pub fn is_missing(&self) -> bool {
        matches!(
            self,
            Self::MissingConversationStream(_) | Self::MissingConversationMetadata(_)
        )
    }
}

#[cfg(test)]
impl PartialEq for LoadErrorInner {
    fn eq(&self, other: &Self) -> bool {
        if std::mem::discriminant(self) != std::mem::discriminant(other) {
            return false;
        }

        // Good enough for testing purposes
        format!("{self:?}") == format!("{other:?}")
    }
}

impl Storage {
    /// Scan conversation index entries from both roots for the given partition.
    ///
    /// IDs are deduplicated across roots, and each entry's [`StoragePresence`]
    /// records which roots hold the conversation.
    /// Entries are sorted by ID.
    #[must_use]
    pub fn load_conversation_index(
        &self,
        filter: ConversationFilter,
    ) -> Vec<ConversationIndexEntry> {
        let partition = |root: &Utf8Path| -> Utf8PathBuf {
            let live = root.join(CONVERSATIONS_DIR);
            if filter.archived {
                live.join(ARCHIVE_DIR)
            } else {
                live
            }
        };

        let workspace_ids: HashSet<ConversationId> = scan_conversation_ids(&partition(&self.root))
            .into_iter()
            .collect();
        let user_ids: HashSet<ConversationId> = self
            .user
            .as_deref()
            .map(|user| {
                scan_conversation_ids(&partition(user))
                    .into_iter()
                    .collect()
            })
            .unwrap_or_default();

        let mut entries: Vec<ConversationIndexEntry> = workspace_ids
            .union(&user_ids)
            .map(|id| ConversationIndexEntry {
                id: *id,
                presence: presence_of(user_ids.contains(id), workspace_ids.contains(id)),
            })
            .collect();
        entries.sort_by_key(|entry| entry.id);
        entries
    }
}

/// The conversation IDs projected in a checkout's storage directory.
///
/// A bare directory scan of the active (non-archived) partition: no workspace
/// construction, no user-local merge, no registry writes.
/// `jp w show` uses this to union checkout-only conversations across a
/// workspace's sibling roots without paying a full workspace load per root —
/// one loaded checkout already contributes every user-local conversation, so
/// the siblings can only add conversations that live in their projection alone.
#[must_use]
pub fn projected_conversation_ids(storage_dir: &Utf8Path) -> Vec<ConversationId> {
    scan_conversation_ids(&storage_dir.join(CONVERSATIONS_DIR))
}

/// Scan a single conversations directory for IDs.
///
/// Dot-prefixed entries (`.trash/`, leftover `.tmp`/`.staging-`/`.old-` dirs
/// from older versions) are skipped by [`load_conversation_id_from_entry`].
fn scan_conversation_ids(path: &Utf8Path) -> Vec<ConversationId> {
    let entries: Vec<_> = dir_entries(path).collect();
    entries
        .par_iter()
        .filter_map(load_conversation_id_from_entry)
        .collect()
}

impl Storage {
    /// Load a conversation's stream, reading `fallback` only if the stored
    /// config cannot be finalized on its own.
    pub fn load_conversation_stream(
        &self,
        id: &ConversationId,
        fallback: &PartialAppConfig,
    ) -> Result<ConversationStream> {
        let workspace_dir = find_conversation_dir_path(&self.root, id);
        let user_dir = self
            .user
            .as_deref()
            .and_then(|user| find_conversation_dir_path(user, id));

        // The stream (`base_config.json` + `events.json`) is one unit: load both
        // files from whichever root has the newer combined mtime, with ties
        // going to the durable user-local copy.
        let Some(conv_dir) =
            pick_newer(user_dir.as_deref(), workspace_dir.as_deref(), stream_mtime)
        else {
            return Err(LoadError::new(
                build_conversation_dir_prefix(&self.root, id),
                LoadErrorInner::MissingConversationStream(*id),
            ));
        };

        let events_path = conv_dir.join(EVENTS_FILE);
        if !events_path.is_file() {
            return Err(LoadError::new(
                build_conversation_dir_prefix(&self.root, id),
                LoadErrorInner::MissingConversationStream(*id),
            ));
        }

        let base_config_path = conv_dir.join(BASE_CONFIG_FILE);
        if base_config_path.is_file() {
            // Current format: separate `base_config.json` and `events.json`.
            let base_config = load_json(&base_config_path)?;
            let events = load_json(&events_path)?;

            return ConversationStream::from_parts(base_config, events, fallback)
                .map(|stream| stream.with_created_at(id.timestamp()))
                .map_err(|error| {
                    LoadError::new(conv_dir.to_owned(), LoadErrorInner::Stream(error))
                });
        }

        // Legacy format: base config packed as first element in events.json.
        let events = load_json(&events_path)?;
        match ConversationStream::from_legacy_events(events, fallback) {
            Ok(Some(stream)) => Ok(stream.with_created_at(id.timestamp())),
            Ok(None) => Err(LoadError::new(
                conv_dir.to_owned(),
                LoadErrorInner::Stream(StreamError::FromEmptyIterator),
            )),
            Err(error) => Err(LoadError::new(
                conv_dir.to_owned(),
                LoadErrorInner::Stream(error),
            )),
        }
    }

    pub fn load_conversation_metadata(&self, id: &ConversationId) -> Result<Conversation> {
        let workspace_dir = find_conversation_dir_path(&self.root, id);
        let user_dir = self
            .user
            .as_deref()
            .and_then(|user| find_conversation_dir_path(user, id));

        self.load_conversation_metadata_from(id, user_dir.as_deref(), workspace_dir.as_deref())
    }

    /// Load a conversation's metadata, given its directory in each root.
    ///
    /// The metadata comes from the directory with the newer `metadata.json`,
    /// and the event count and last activity from the directory with the newer
    /// stream, so the two can come from different roots.
    /// Ties go to user-local storage.
    fn load_conversation_metadata_from(
        &self,
        id: &ConversationId,
        user_dir: Option<&Utf8Path>,
        workspace_dir: Option<&Utf8Path>,
    ) -> Result<Conversation> {
        // Metadata resolves on its own mtime, independently of the stream.
        let meta_dir = pick_newer(user_dir, workspace_dir, |dir| {
            file_mtime(&dir.join(METADATA_FILE))
        });
        let Some(meta_dir) = meta_dir else {
            return Err(LoadError::new(
                build_conversation_dir_prefix(&self.root, id),
                LoadErrorInner::MissingConversationMetadata(*id),
            ));
        };

        let meta_path = meta_dir.join(METADATA_FILE);
        if !meta_path.is_file() {
            return Err(LoadError::new(
                build_conversation_dir_prefix(&self.root, id),
                LoadErrorInner::MissingConversationMetadata(*id),
            ));
        }

        let mut conversation: Conversation = load_json(&meta_path)?;

        // Event count and last activity describe the stream, so read them from
        // the stream root (which may differ from the metadata root).
        if let Some(stream_dir) = pick_newer(user_dir, workspace_dir, stream_mtime) {
            let summary = self.events_summary(stream_dir);
            conversation.events_count = summary.count;
            conversation.last_event_at = summary.last_event_at;
        }

        Ok(conversation)
    }

    /// Load metadata for many conversations from a single directory scan.
    ///
    /// Each conversation directory is resolved once from one enumeration per
    /// root, rather than re-scanning the conversations directory per id (which
    /// is quadratic across the whole index).
    /// The id-to-path map lives only for the duration of this call.
    ///
    /// On a map miss or a stale path (a concurrent persist renamed the
    /// directory between the scan and the read), the affected id falls back to
    /// [`Self::load_conversation_metadata`], which rescans with the
    /// in-flight-persist retry.
    pub fn load_conversation_metadata_batch(
        &self,
        ids: &[ConversationId],
    ) -> Vec<(ConversationId, Result<Conversation>)> {
        let workspace_dirs = conversation_dirs_by_id(&self.root);
        let user_dirs = self
            .user
            .as_deref()
            .map(conversation_dirs_by_id)
            .unwrap_or_default();

        ids.par_iter()
            .map(|id| {
                // Fast path: resolve from the listings above. On any miss or
                // failure (stale path, corrupt active metadata), fall back to
                // the rescanning loader, including the archive, to match
                // `FsStorageBackend::load_conversation_metadata`.
                let result = self
                    .load_conversation_metadata_from(
                        id,
                        user_dirs.get(id).map(Utf8PathBuf::as_path),
                        workspace_dirs.get(id).map(Utf8PathBuf::as_path),
                    )
                    .or_else(|_| self.load_conversation_metadata(id))
                    .or_else(|_| self.load_archived_conversation_metadata(id));

                (*id, result)
            })
            .collect()
    }

    /// Return the event count and last-event timestamp for `events.json` in
    /// `dir`.
    ///
    /// Uses the cached summary from the last validation scan if the file's size
    /// and modification time still match; otherwise parses `events.json`.
    /// Returns an empty [`EventsSummary`] if the file is missing or invalid.
    pub(crate) fn events_summary(&self, dir: &Utf8Path) -> EventsSummary {
        let path = dir.join(EVENTS_FILE);
        if let Some(stamp) = FileStamp::of(&path)
            && let Some(summary) = self.scan.summary(dir, stamp)
        {
            return summary;
        }

        let Ok(bytes) = fs::read(&path) else {
            return EventsSummary::default();
        };

        EventsSummary::parse(&bytes).unwrap_or_else(|error| {
            warn!(
                error = error.to_string(),
                path = path.as_str(),
                "Error parsing JSON event file."
            );
            EventsSummary::default()
        })
    }
}

/// Map each [`ConversationId`] under `root`'s conversations directory to its
/// directory path.
///
/// If multiple directories share an ID, keeps the first entry returned by
/// [`dir_entries`], matching [`find_conversation_dir_path`].
fn conversation_dirs_by_id(root: &Utf8Path) -> HashMap<ConversationId, Utf8PathBuf> {
    let mut dirs = HashMap::new();
    for entry in dir_entries(root.join(CONVERSATIONS_DIR)) {
        if let Some(id) = load_conversation_id_from_entry(&entry) {
            dirs.entry(id).or_insert_with(|| entry.into_path());
        }
    }

    dirs
}

/// Modification time of a single file, or the epoch when it is absent or
/// unreadable.
fn file_mtime(path: &Utf8Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Combined modification time of a conversation's stream files.
///
/// `base_config.json` is written once but independently user-editable, so the
/// stream's freshness is the newer of the two files.
/// Legacy conversations have no `base_config.json`, leaving just the
/// `events.json` mtime.
fn stream_mtime(conv_dir: &Utf8Path) -> SystemTime {
    file_mtime(&conv_dir.join(EVENTS_FILE)).max(file_mtime(&conv_dir.join(BASE_CONFIG_FILE)))
}

/// Pick the directory with the newer mtime, preferring user-local on a tie.
///
/// With no evidence the workspace projection is newer, the durable user-local
/// copy stays authoritative.
fn pick_newer<'a>(
    user_dir: Option<&'a Utf8Path>,
    workspace_dir: Option<&'a Utf8Path>,
    mtime: impl Fn(&Utf8Path) -> SystemTime,
) -> Option<&'a Utf8Path> {
    match (user_dir, workspace_dir) {
        (Some(user), Some(workspace)) => Some(if mtime(user) >= mtime(workspace) {
            user
        } else {
            workspace
        }),
        (Some(user), None) => Some(user),
        (None, workspace) => workspace,
    }
}

/// Classify a conversation's [`StoragePresence`] from root membership.
fn presence_of(in_user: bool, in_workspace: bool) -> StoragePresence {
    match (in_user, in_workspace) {
        (true, true) => StoragePresence::Projected,
        (true, false) => StoragePresence::UserLocalOnly,
        _ => StoragePresence::WorkspaceOnly,
    }
}

/// Read and deserialize a JSON file, mapping errors to [`LoadError`].
///
/// Reads the file whole and parses the slice rather than streaming it.
/// `serde_json`'s reader-based path advances one byte at a time and so loses
/// the `memchr` scans and zero-copy string borrows it gets over a contiguous
/// buffer — measurably slower on the conversation streams this loads.
pub(crate) fn load_json<T: DeserializeOwned>(path: &Utf8Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|error| LoadError {
        path: path.to_path_buf(),
        error: error.into(),
    })?;

    serde_json::from_slice(&bytes).map_err(|error| LoadError {
        path: path.to_path_buf(),
        error: error.into(),
    })
}
