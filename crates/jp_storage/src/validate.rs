use std::{fmt, fs, io, time::SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use jp_conversation::ConversationId;
use rayon::prelude::*;
use serde::de::IgnoredAny;
use tracing::{debug, trace};

use crate::{
    BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, IMPORT_STAGING_PREFIX, METADATA_FILE,
    Storage, dir_entries, holds_managed_files,
    scan::{DirStamps, EventsSummary, SCAN_CACHE_FILE, ScanCache, ScannedDir},
    value::{cleanup_tmp_files, has_tmp_files},
};

/// Result of validating all conversation directories across storage roots.
#[derive(Debug, Default)]
pub struct ValidationResult {
    /// Conversations that passed all validation checks.
    pub valid: Vec<ValidConversation>,

    /// Directories named for a conversation that hold none of the managed
    /// files.
    pub unstored: Vec<UnstoredConversation>,

    /// Conversations that failed one or more validation checks.
    pub invalid: Vec<InvalidConversation>,

    /// Directories under `conversations/` whose name is not a conversation ID.
    ///
    /// They are not conversations, so their contents are not checked.
    pub unrecognized: Vec<Utf8PathBuf>,
}

/// A conversation directory that passed validation.
#[derive(Debug)]
pub struct ValidConversation {
    /// The parsed conversation ID.
    pub id: ConversationId,

    /// The directory name on disk.
    pub dirname: String,
}

/// A directory named for a conversation whose first write never happened.
///
/// It holds none of the managed files (`metadata.json`, `base_config.json`,
/// `events.json`), only whatever was put there before the conversation was
/// stored, such as the query draft of a run that ended before its first write.
/// It is not a conversation: it is never indexed, never loaded, and never
/// reported as a validation failure.
#[derive(Debug)]
pub struct UnstoredConversation {
    pub id: ConversationId,
    pub path: Utf8PathBuf,
}

/// A conversation directory that failed validation.
#[derive(Debug)]
pub struct InvalidConversation {
    /// The conversations root this entry lives under.
    pub(crate) conversations_dir: Utf8PathBuf,

    pub id: ConversationId,

    /// What went wrong.
    pub error: ValidationError,

    /// The directory name on disk.
    pub dirname: String,
}

/// The reason a conversation directory failed validation.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// A required managed file is missing.
    ///
    /// `metadata.json` and `events.json` are required; `base_config.json` is
    /// optional, because older conversations pack the base config into
    /// `events.json`.
    #[error("missing {file}")]
    MissingFile { file: ManagedFile },

    /// A managed file exists but could not be read or does not parse.
    #[error("{file}: {source}")]
    UnreadableFile {
        file: ManagedFile,
        #[source]
        source: FileError,
    },
}

impl ValidationError {
    /// The managed file the failure is about.
    #[must_use]
    pub const fn file(&self) -> ManagedFile {
        match self {
            Self::MissingFile { file } | Self::UnreadableFile { file, .. } => *file,
        }
    }
}

/// A file storage writes and owns inside a conversation directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedFile {
    Metadata,
    BaseConfig,
    Events,
}

impl ManagedFile {
    /// The file's name inside the conversation directory.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Metadata => METADATA_FILE,
            Self::BaseConfig => BASE_CONFIG_FILE,
            Self::Events => EVENTS_FILE,
        }
    }
}

impl fmt::Display for ManagedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a managed file that exists failed validation.
#[derive(Debug, thiserror::Error)]
pub enum FileError {
    /// The file could not be read.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// The file is not JSON, or not JSON of the shape the file holds.
    #[error("invalid JSON at line {}, column {}", .0.line(), .0.column())]
    Json(#[from] serde_json::Error),
}

impl InvalidConversation {
    /// The conversation directory that failed validation.
    #[must_use]
    pub fn path(&self) -> Utf8PathBuf {
        self.conversations_dir.join(&self.dirname)
    }
}

impl fmt::Display for InvalidConversation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.dirname, self.error)
    }
}

impl Storage {
    /// Scan all conversation directories and check structural integrity.
    ///
    /// This is a lightweight check that runs on every startup.
    /// A directory whose name does not parse as a [`ConversationId`] is not a
    /// conversation, and is listed in [`ValidationResult::unrecognized`]
    /// without being checked.
    /// For every other directory it verifies:
    ///
    /// 1. A `metadata.json` file exists and is valid JSON.
    /// 2. An `events.json` file exists and is a JSON array where each element
    ///    contains `timestamp` and `type` fields.
    ///
    /// Field values are skipped via [`IgnoredAny`] without allocating.
    /// Content-level issues (bad field values, missing optional fields, schema
    /// mismatches) are handled at load time by
    /// [`ConversationStream::sanitize`].
    ///
    /// Directories whose files match the size and modification time from the
    /// last passing scan are skipped without re-reading the files.
    /// Scan records are cached in memory and persisted to user-local storage
    /// across processes.
    /// Directories with a file modified within two seconds before the scan are
    /// not cached, so the next scan re-reads them.
    ///
    /// Files and dot-prefixed directories (e.g., `.trash/`) are silently
    /// skipped.
    ///
    /// [`ConversationStream::sanitize`]: jp_conversation::ConversationStream::sanitize
    /// [`IgnoredAny`]: serde::de::IgnoredAny
    #[must_use]
    pub fn validate_conversations(&self) -> ValidationResult {
        self.validate_conversations_at(SystemTime::now())
    }

    /// Run [`Self::validate_conversations`] as a scan started at
    /// `scan_started`.
    fn validate_conversations_at(&self, scan_started: SystemTime) -> ValidationResult {
        trace!("Validating conversations.");

        let cache_path = self.load_scan_cache();

        let mut result = ValidationResult::default();
        let mut scanned = Vec::new();

        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let conversations_dir = root.join(CONVERSATIONS_DIR);
            if !conversations_dir.is_dir() {
                continue;
            }

            self.cleanup_import_staging(&conversations_dir);
            self.cleanup_orphaned_tmp_files(&conversations_dir);
            validate_root(&conversations_dir, &self.scan, &mut result, &mut scanned);
        }

        self.scan.extend(scanned.iter().cloned(), scan_started);
        if let Some(path) = &cache_path {
            self.scan.save(path, scanned, scan_started);
        }

        debug!(
            valid = result.valid.len(),
            invalid = result.invalid.len(),
            "Validated conversations.",
        );

        result
    }

    /// IDs of the conversations under `conversations_dir` that pass
    /// [`Self::validate_conversations`]'s checks.
    ///
    /// Reads nothing on disk but file stamps for a directory the scan cache
    /// already records, and records the directories it reads.
    /// Removes nothing: unlike [`Self::validate_conversations`], it leaves
    /// leftovers of interrupted writes for the next validation.
    pub(crate) fn valid_conversation_ids(
        &self,
        conversations_dir: &Utf8Path,
    ) -> Vec<ConversationId> {
        let scan_started = SystemTime::now();
        self.load_scan_cache();

        let mut result = ValidationResult::default();
        let mut scanned = Vec::new();
        validate_root(conversations_dir, &self.scan, &mut result, &mut scanned);
        self.scan.extend(scanned, scan_started);

        result.valid.into_iter().map(|valid| valid.id).collect()
    }

    /// Load the persisted scan cache into memory, and return where it lives.
    ///
    /// `None` without user-local storage, which is where the cache is kept.
    fn load_scan_cache(&self) -> Option<Utf8PathBuf> {
        let path = self
            .user
            .as_ref()
            .map(|user| user.join(CONVERSATIONS_DIR))
            .filter(|dir| dir.is_dir())
            .map(|dir| dir.join(SCAN_CACHE_FILE))?;

        self.scan.load(&path);
        Some(path)
    }

    /// Remove leftover import staging directories from one `conversations/`
    /// directory.
    ///
    /// A staging directory only survives a crash between an import's copy and
    /// its rename, and holds nothing the workspace copy does not still have, so
    /// removing one is pure disk hygiene: each import stages under a fresh name
    /// and never reads a leftover.
    ///
    /// Each removal is made while holding the conversation's own lock.
    /// Without it, this would race an import running under that lock in another
    /// process: `copy_dir_all` recreates directories as it recurses, so a
    /// mid-copy removal would leave the importer to rename an incomplete tree
    /// into place.
    /// A conversation whose lock is held is skipped entirely, and because the
    /// sweep is only reclaiming disk space, skipping costs nothing but a delay
    /// until the next run.
    fn cleanup_import_staging(&self, conversations_dir: &Utf8Path) {
        for entry in dir_entries(conversations_dir) {
            let Some(dirname) = entry.file_name().strip_prefix(IMPORT_STAGING_PREFIX) else {
                continue;
            };
            if !entry.path().is_dir() {
                continue;
            }

            // An unparseable name cannot be tied to a lock, so leave it be
            // rather than remove a directory whose owner is unknown.
            let Ok(id) = ConversationId::try_from_dirname(dirname) else {
                trace!(path = %entry.path(), "Skipping unparseable staging directory.");
                continue;
            };

            let Ok(Some(_guard)) = self.try_lock_conversation(&id.to_string(), None) else {
                trace!(%id, "Skipping staging directory: conversation is locked.");
                continue;
            };

            let path = entry.into_path();
            trace!(%path, "Removing leftover import staging directory.");
            let _err = std::fs::remove_dir_all(path);
        }
    }

    /// Remove orphaned `.tmp` files from conversation directories under
    /// `conversations_dir`.
    ///
    /// Acquires each conversation's lock before deleting its `.tmp` files so
    /// the sweep does not delete the staging file of an in-flight atomic write
    /// in another process.
    /// Conversations that are currently locked are skipped.
    fn cleanup_orphaned_tmp_files(&self, conversations_dir: &Utf8Path) {
        for entry in dir_entries(conversations_dir) {
            if !entry.file_type().is_ok_and(|ft| ft.is_dir())
                || entry.file_name().starts_with('.')
                || !has_tmp_files(entry.path())
            {
                continue;
            }

            // A directory whose name is not a conversation ID cannot be
            // locked, and is not JP's to clean up.
            let Ok(id) = ConversationId::try_from_dirname(entry.file_name()) else {
                continue;
            };

            let Ok(Some(_guard)) = self.try_lock_conversation(&id.to_string(), None) else {
                trace!(%id, "Skipping temp file cleanup: conversation is locked.");
                continue;
            };

            cleanup_tmp_files(entry.path());
        }
    }
}

/// Validate every conversation directory under `conversations_dir`.
///
/// Directories that pass validation and have complete file stamps are appended
/// to `scanned`.
fn validate_root(
    conversations_dir: &Utf8Path,
    cache: &ScanCache,
    result: &mut ValidationResult,
    scanned: &mut Vec<(Utf8PathBuf, ScannedDir)>,
) {
    trace!(root = %conversations_dir, "Validating conversation root.");

    let entries: Vec<_> = dir_entries(conversations_dir)
        .filter(|entry| {
            entry.file_type().ok().is_some_and(|ft| ft.is_dir())
                && !entry.file_name().starts_with('.')
        })
        .collect();

    let mut named = vec![];
    for entry in entries {
        match ConversationId::try_from_dirname(entry.file_name()) {
            Ok(id) => named.push((entry, id)),
            Err(_) => result.unrecognized.push(entry.into_path()),
        }
    }

    let outcomes: Vec<_> = named
        .par_iter()
        .map(|(entry, id)| {
            validate_entry(
                conversations_dir,
                entry.file_name(),
                *id,
                entry.path(),
                cache,
            )
        })
        .collect();

    for ((entry, _), outcome) in named.into_iter().zip(outcomes) {
        match outcome {
            Ok(Validated::Stored(valid, scan)) => {
                result.valid.push(valid);
                if let Some(scan) = scan {
                    scanned.push((entry.into_path(), scan));
                }
            }
            Ok(Validated::Unstored(unstored)) => result.unstored.push(unstored),
            Err(e) => result.invalid.push(e),
        }
    }
}

/// A conversation directory that passed validation.
enum Validated {
    /// A stored conversation, with its scan record when its file stamps are
    /// complete.
    Stored(ValidConversation, Option<ScannedDir>),

    /// A directory holding none of the managed files.
    Unstored(UnstoredConversation),
}

/// What a conversation directory that passes validation holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirContents {
    /// A stored conversation.
    Stored,

    /// Files, but none of the managed ones; see [`UnstoredConversation`].
    Unstored,

    /// Nothing at all.
    Empty,
}

/// Check one conversation directory's files the way startup validation checks
/// each directory under `conversations/`.
///
/// The directory can live anywhere and be named anything.
/// Reads the directory's files and changes nothing on disk.
///
/// # Errors
///
/// Returns the reason the directory fails validation.
pub fn validate_conversation_dir(dir: &Utf8Path) -> Result<DirContents, ValidationError> {
    check_files(dir, &ScanCache::default()).map(|(contents, _)| contents)
}

/// Validate a single conversation directory entry named for `id`.
fn validate_entry(
    conversations_dir: &Utf8Path,
    dirname: &str,
    id: ConversationId,
    entry_path: &Utf8Path,
    cache: &ScanCache,
) -> Result<Validated, InvalidConversation> {
    match check_files(entry_path, cache) {
        Ok((DirContents::Stored, scan)) => Ok(Validated::Stored(
            ValidConversation {
                id,
                dirname: dirname.to_owned(),
            },
            scan,
        )),
        Ok((DirContents::Unstored | DirContents::Empty, _)) => {
            Ok(Validated::Unstored(UnstoredConversation {
                id,
                path: entry_path.to_path_buf(),
            }))
        }
        Err(error) => Err(InvalidConversation {
            conversations_dir: conversations_dir.to_path_buf(),
            id,
            error,
            dirname: dirname.to_owned(),
        }),
    }
}

/// Check the managed files in `dir`.
///
/// Returns [`ScannedDir`] alongside a stored conversation when `metadata.json`
/// and `events.json` both have valid file stamps.
fn check_files(
    dir: &Utf8Path,
    cache: &ScanCache,
) -> Result<(DirContents, Option<ScannedDir>), ValidationError> {
    if !holds_managed_files(dir) {
        let empty = dir
            .read_dir_utf8()
            .is_ok_and(|mut entries| entries.next().is_none());
        let contents = if empty {
            DirContents::Empty
        } else {
            DirContents::Unstored
        };
        return Ok((contents, None));
    }

    // Capture file stamps before reading any files so a mid-check write keeps
    // the older stamp and is re-read on the next scan.
    let stamps = DirStamps::of(dir);
    if let Some(scan) = cache.get(dir, &stamps) {
        return Ok((DirContents::Stored, Some(scan)));
    }

    let unreadable = |file, source| ValidationError::UnreadableFile { file, source };

    let metadata_path = dir.join(METADATA_FILE);
    if !metadata_path.is_file() {
        return Err(ValidationError::MissingFile {
            file: ManagedFile::Metadata,
        });
    }
    validate_metadata(&metadata_path)
        .map_err(|source| unreadable(ManagedFile::Metadata, source))?;

    // base_config.json is optional for backward compat (old conversations have
    // the base config packed into events.json), but if it exists it must be
    // valid JSON.
    let base_config_path = dir.join(BASE_CONFIG_FILE);
    if base_config_path.is_file() {
        validate_json_file(&base_config_path)
            .map_err(|source| unreadable(ManagedFile::BaseConfig, source))?;
    }

    let events_path = dir.join(EVENTS_FILE);
    if !events_path.is_file() {
        return Err(ValidationError::MissingFile {
            file: ManagedFile::Events,
        });
    }
    let summary =
        validate_events(&events_path).map_err(|source| unreadable(ManagedFile::Events, source))?;

    Ok((DirContents::Stored, stamps.scanned(summary)))
}

/// Confirm `metadata.json` is a valid JSON object.
///
/// Uses [`IgnoredAny`] to skip all field values — no allocations, no schema
/// checks.
/// Content validation happens at load time.
///
/// [`IgnoredAny`]: serde::de::IgnoredAny
fn validate_metadata(path: &Utf8Path) -> Result<(), FileError> {
    validate_json_file(path)
}

/// Confirm a file is valid JSON.
fn validate_json_file(path: &Utf8Path) -> Result<(), FileError> {
    let buf = fs::read(path)?;
    serde_json::from_slice::<IgnoredAny>(&buf)?;
    Ok(())
}

/// Confirm `events.json` is a JSON array of objects with `timestamp` and `type`
/// fields, and summarize it from the same parse.
///
/// All elements in the array (both `ConfigDelta` and `ConversationEvent`)
/// serialize with `timestamp` and `type` as top-level fields.
/// `type` is the tag for the flattened `EventKind` enum on events, and an
/// explicit `"config_delta"` tag on config deltas.
/// This confirms the structural shape without materializing any event data.
fn validate_events(path: &Utf8Path) -> Result<EventsSummary, FileError> {
    let buf = fs::read(path)?;
    Ok(EventsSummary::parse(&buf)?)
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
