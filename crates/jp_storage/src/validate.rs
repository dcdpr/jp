use std::{fmt, fs, io, time::SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use jp_conversation::ConversationId;
use rayon::prelude::*;
use serde::de::IgnoredAny;
use tracing::{debug, trace};

pub use crate::location::StorageLocation;
use crate::{
    ARCHIVE_DIR, BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, IMPORT_STAGING_PREFIX,
    METADATA_FILE, Storage, dir_entries,
    error::Error,
    scan::{DirStamps, EventsSummary, SCAN_CACHE_FILE, ScanCache, ScannedDir},
    value::{cleanup_tmp_files, has_tmp_files, tmp_files},
};

/// Result of validating all conversation directories across storage roots.
#[derive(Debug, Default)]
pub struct ValidationResult {
    /// Conversations that passed all validation checks.
    pub valid: Vec<ValidConversation>,

    /// Conversations that failed one or more validation checks.
    pub invalid: Vec<InvalidConversation>,

    /// Directories under `conversations/` whose name is not a conversation ID.
    pub unrecognized: Vec<Utf8PathBuf>,
}

/// Output of [`Storage::diagnose_conversations`].
#[derive(Debug, Default)]
pub(crate) struct ConversationsDiagnosis {
    /// Conversation directories that fail validation, sorted by path.
    pub(crate) invalid: Vec<InvalidConversation>,

    /// Directories under `conversations/` whose name is not a conversation ID,
    /// sorted by path.
    pub(crate) unrecognized: Vec<Utf8PathBuf>,

    /// Orphaned `.tmp` files and import staging directories left behind by
    /// interrupted writes, sorted by path.
    pub(crate) leftovers: Vec<Utf8PathBuf>,
}

/// A conversation directory that passed validation.
#[derive(Debug)]
pub struct ValidConversation {
    pub id: ConversationId,
    pub dirname: String,
}

/// A conversation directory that failed validation.
#[derive(Debug)]
pub struct InvalidConversation {
    /// The conversations root this entry lives under.
    pub(crate) conversations_dir: Utf8PathBuf,

    pub id: ConversationId,
    pub archived: bool,
    pub error: ValidationError,
    pub dirname: String,
}

/// Reason a stored conversation failed validation.
///
/// Display output uses the part location's [`StorageLocation::label`] (its file
/// name on the filesystem backend).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// A required conversation file (`metadata.json` or `events.json`) is
    /// missing.
    #[error("missing {}", .location.label())]
    Missing {
        part: ConversationPart,
        location: StorageLocation,
    },

    /// A conversation part exists but could not be read or parsed.
    #[error("{}: {error}", .location.label())]
    Unreadable {
        part: ConversationPart,
        location: StorageLocation,
        #[source]
        error: ReadError,
    },
}

impl ValidationError {
    #[must_use]
    pub const fn part(&self) -> ConversationPart {
        match self {
            Self::Missing { part, .. } | Self::Unreadable { part, .. } => *part,
        }
    }

    #[must_use]
    pub const fn location(&self) -> &StorageLocation {
        match self {
            Self::Missing { location, .. } | Self::Unreadable { location, .. } => location,
        }
    }
}

/// A part of a stored conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationPart {
    Metadata,
    BaseConfig,
    Events,
}

impl ConversationPart {
    /// Stable machine-readable identifier for this part.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::BaseConfig => "base_config",
            Self::Events => "events",
        }
    }

    /// On-disk file name for this part.
    pub(crate) const fn file_name(self) -> &'static str {
        match self {
            Self::Metadata => METADATA_FILE,
            Self::BaseConfig => BASE_CONFIG_FILE,
            Self::Events => EVENTS_FILE,
        }
    }
}

/// Reason an existing conversation part could not be read or parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The part could not be read from storage (for example, due to a
    /// permission error).
    #[error("{0}")]
    Unavailable(String),

    /// The part's content could not be parsed.
    #[error("{}{}", malformed(*format), at(*position))]
    Malformed {
        /// Data format name shown in error messages (such as `"JSON"`).
        format: Option<&'static str>,

        message: String,

        /// 1-based line and column of the parse error, for text formats.
        position: Option<Position>,
    },
}

/// 1-based line and column position in a text file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

fn malformed(format: Option<&str>) -> String {
    format.map_or_else(
        || "malformed".to_owned(),
        |format| format!("invalid {format}"),
    )
}

fn at(position: Option<Position>) -> String {
    position.map_or_else(String::new, |p| {
        format!(" at line {}, column {}", p.line, p.column)
    })
}

impl From<io::Error> for ReadError {
    fn from(error: io::Error) -> Self {
        Self::Unavailable(error.to_string())
    }
}

impl From<serde_json::Error> for ReadError {
    fn from(error: serde_json::Error) -> Self {
        Self::Malformed {
            format: Some("JSON"),
            message: error.to_string(),
            position: Some(Position {
                line: error.line(),
                column: error.column(),
            }),
        }
    }
}

impl InvalidConversation {
    #[must_use]
    pub fn path(&self) -> Utf8PathBuf {
        self.conversations_dir.join(&self.dirname)
    }

    #[must_use]
    pub fn location(&self) -> StorageLocation {
        StorageLocation::of_path(&self.path())
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

    /// Validate live and archived conversation directories and collect leftover
    /// temporary files without modifying the store.
    ///
    /// Unlike [`Self::validate_conversations`], this method checks archived
    /// conversations, returns orphaned `.tmp` files and import staging
    /// directories instead of deleting them, and reads the scan cache without
    /// updating it on disk.
    ///
    /// # Errors
    ///
    /// Returns an error if a conversations or archive directory exists but
    /// cannot be listed.
    pub(crate) fn diagnose_conversations(&self) -> Result<ConversationsDiagnosis, Error> {
        trace!("Diagnosing conversations.");
        self.load_scan_cache();

        let mut validation = ValidationResult::default();
        let mut leftovers = vec![];
        for root in [Some(&self.root), self.user.as_ref()].into_iter().flatten() {
            let conversations_dir = root.join(CONVERSATIONS_DIR);
            if !listable(&conversations_dir)? {
                continue;
            }

            leftovers.extend(leftovers_in(&conversations_dir));
            validate_root(&conversations_dir, &self.scan, &mut validation, &mut vec![]);
        }

        let mut invalid = validation.invalid;
        invalid.extend(self.validate_archive()?);
        invalid.sort_by_key(InvalidConversation::path);
        let mut unrecognized = validation.unrecognized;
        unrecognized.sort();
        leftovers.sort();

        Ok(ConversationsDiagnosis {
            invalid,
            unrecognized,
            leftovers,
        })
    }

    /// Validate archived conversation directories without modifying the store.
    ///
    /// Reads the scan cache without writing to it.
    ///
    /// # Errors
    ///
    /// Returns an error if an archive directory exists but cannot be listed.
    pub(crate) fn validate_archive(&self) -> Result<Vec<InvalidConversation>, Error> {
        let mut archive = ValidationResult::default();
        for root in [Some(&self.root), self.user.as_ref()].into_iter().flatten() {
            let archive_dir = root.join(CONVERSATIONS_DIR).join(ARCHIVE_DIR);
            if listable(&archive_dir)? {
                validate_root(&archive_dir, &self.scan, &mut archive, &mut vec![]);
            }
        }

        Ok(archive.invalid)
    }

    /// Return the IDs of conversations under `conversations_dir` that pass
    /// [`Self::validate_conversations`].
    ///
    /// Uses and updates the in-memory scan cache, but does not clean up
    /// leftover temporary files or staging directories.
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

/// Collect leftover import staging directories and orphaned `.tmp` files under
/// `conversations_dir`.
///
/// Ignores directories whose name does not parse as a [`ConversationId`].
fn leftovers_in(conversations_dir: &Utf8Path) -> Vec<Utf8PathBuf> {
    let mut leftovers = vec![];
    for entry in dir_entries(conversations_dir) {
        if !entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }

        let name = entry.file_name();
        if let Some(dirname) = name.strip_prefix(IMPORT_STAGING_PREFIX) {
            if ConversationId::try_from_dirname(dirname).is_ok() {
                leftovers.push(entry.into_path());
            }
        } else if ConversationId::try_from_dirname(name).is_ok() {
            leftovers.extend(tmp_files(entry.path()));
        }
    }

    leftovers
}

/// Return `true` if `dir` exists and can be listed, or `false` if it does not
/// exist.
///
/// # Errors
///
/// Returns [`Error::ReadFailed`] if `dir` exists but cannot be listed.
fn listable(dir: &Utf8Path) -> Result<bool, Error> {
    match dir.read_dir_utf8() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(Error::ReadFailed {
            path: dir.to_owned(),
            source,
        }),
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
            Ok((valid, scan)) => {
                result.valid.push(valid);
                if let Some(scan) = scan {
                    scanned.push((entry.into_path(), scan));
                }
            }
            Err(e) => result.invalid.push(e),
        }
    }
}

/// Validate the conversation files in `dir` without modifying the filesystem.
///
/// # Errors
///
/// Returns the [`ValidationError`] if any required file is missing or invalid.
pub fn validate_conversation_dir(dir: &Utf8Path) -> Result<(), ValidationError> {
    check_files(dir, &ScanCache::default()).map(|_| ())
}

/// Return `true` if `dir` is missing `metadata.json` and all other conversation
/// files are valid.
pub(crate) fn missing_only_metadata(dir: &Utf8Path) -> bool {
    !dir.join(METADATA_FILE).exists() && check_contents(dir).is_ok()
}

/// Validate a single conversation directory entry named for `id`.
fn validate_entry(
    conversations_dir: &Utf8Path,
    dirname: &str,
    id: ConversationId,
    entry_path: &Utf8Path,
    cache: &ScanCache,
) -> Result<(ValidConversation, Option<ScannedDir>), InvalidConversation> {
    match check_files(entry_path, cache) {
        Ok(scan) => Ok((
            ValidConversation {
                id,
                dirname: dirname.to_owned(),
            },
            scan,
        )),
        Err(error) => Err(InvalidConversation {
            conversations_dir: conversations_dir.to_path_buf(),
            id,
            archived: conversations_dir.file_name() == Some(ARCHIVE_DIR),
            error,
            dirname: dirname.to_owned(),
        }),
    }
}

/// Check the conversation files in `dir`.
///
/// Returns [`ScannedDir`] when `metadata.json` and `events.json` both have
/// valid file stamps.
fn check_files(dir: &Utf8Path, cache: &ScanCache) -> Result<Option<ScannedDir>, ValidationError> {
    // Capture file stamps before reading any files so a mid-check write keeps
    // the older stamp and is re-read on the next scan.
    let stamps = DirStamps::of(dir);
    if let Some(scan) = cache.get(dir, &stamps) {
        return Ok(Some(scan));
    }

    let metadata_path = dir.join(METADATA_FILE);
    if !metadata_path.is_file() {
        return Err(missing(dir, ConversationPart::Metadata));
    }
    validate_metadata(&metadata_path)
        .map_err(|error| unreadable(dir, ConversationPart::Metadata, error))?;

    let summary = check_contents(dir)?;
    Ok(stamps.scanned(summary))
}

/// Check the conversation files in `dir` other than the metadata, and summarize
/// the events.
fn check_contents(dir: &Utf8Path) -> Result<EventsSummary, ValidationError> {
    // base_config.json is optional for backward compat (old conversations have
    // the base config packed into events.json), but if it exists it must be
    // valid JSON.
    let base_config_path = dir.join(BASE_CONFIG_FILE);
    if base_config_path.is_file() {
        validate_json_file(&base_config_path)
            .map_err(|error| unreadable(dir, ConversationPart::BaseConfig, error))?;
    }

    let events_path = dir.join(EVENTS_FILE);
    if !events_path.is_file() {
        return Err(missing(dir, ConversationPart::Events));
    }
    validate_events(&events_path).map_err(|error| unreadable(dir, ConversationPart::Events, error))
}

fn missing(dir: &Utf8Path, part: ConversationPart) -> ValidationError {
    ValidationError::Missing {
        part,
        location: StorageLocation::of_path(&dir.join(part.file_name())),
    }
}

fn unreadable(dir: &Utf8Path, part: ConversationPart, error: ReadError) -> ValidationError {
    ValidationError::Unreadable {
        part,
        location: StorageLocation::of_path(&dir.join(part.file_name())),
        error,
    }
}

/// Confirm `metadata.json` is valid JSON.
///
/// Uses [`IgnoredAny`] to skip all field values without allocating or checking
/// the schema.
/// Content validation happens at load time.
///
/// [`IgnoredAny`]: serde::de::IgnoredAny
fn validate_metadata(path: &Utf8Path) -> Result<(), ReadError> {
    validate_json_file(path)
}

/// Confirm a file is valid JSON.
fn validate_json_file(path: &Utf8Path) -> Result<(), ReadError> {
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
pub(crate) fn validate_events(path: &Utf8Path) -> Result<EventsSummary, ReadError> {
    let buf = fs::read(path)?;
    Ok(EventsSummary::parse(&buf)?)
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
