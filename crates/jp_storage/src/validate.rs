use std::{fmt, fs, io, time::SystemTime};

use camino::{Utf8Path, Utf8PathBuf};
use jp_conversation::{Conversation, ConversationId};
use rayon::prelude::*;
use serde::de::IgnoredAny;
use tracing::{debug, trace};

pub use crate::location::StorageLocation;
use crate::{
    BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, IMPORT_STAGING_PREFIX, METADATA_FILE,
    Storage, dir_entries,
    scan::{DirStamps, EventsSummary, SCAN_CACHE_FILE, ScanCache, ScannedDir},
    value::{cleanup_tmp_files, has_tmp_files, tmp_files, write_json},
};

/// Result of validating all conversation directories across storage roots.
#[derive(Debug, Default)]
pub struct ValidationResult {
    /// Conversations that passed all validation checks.
    pub valid: Vec<ValidConversation>,

    /// Conversations that failed one or more validation checks.
    pub invalid: Vec<InvalidConversation>,

    /// Directories under `conversations/` whose name is not a conversation ID.
    ///
    /// They are not conversations, so their contents are not checked.
    pub unrecognized: Vec<Utf8PathBuf>,
}

/// What [`Storage::diagnose_conversations`] finds.
#[derive(Debug, Default)]
pub(crate) struct ConversationsDiagnosis {
    /// Conversation directories that fail validation, sorted by path.
    pub(crate) invalid: Vec<InvalidConversation>,

    /// Directories under `conversations/` whose name is not a conversation ID,
    /// sorted.
    pub(crate) unrecognized: Vec<Utf8PathBuf>,

    /// `.tmp` files and import staging directories that interrupted writes left
    /// behind, sorted.
    pub(crate) leftovers: Vec<Utf8PathBuf>,
}

/// A conversation directory that passed validation.
#[derive(Debug)]
pub struct ValidConversation {
    /// The parsed conversation ID.
    pub id: ConversationId,

    /// The directory name on disk.
    pub dirname: String,
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

/// The reason a stored conversation failed validation.
///
/// The message names the part by its location's label: for the filesystem
/// backend, its file name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// A required part of the conversation is missing.
    ///
    /// The metadata and the events are required; the base config is optional,
    /// because older conversations store it with the events.
    #[error("missing {}", .location.label())]
    Missing {
        part: ConversationPart,

        /// Where the part was expected.
        location: StorageLocation,
    },

    /// A part of the conversation exists but could not be read.
    #[error("{}: {error}", .location.label())]
    Unreadable {
        part: ConversationPart,
        location: StorageLocation,
        #[source]
        error: ReadError,
    },
}

impl ValidationError {
    /// The part of the conversation the failure is about.
    #[must_use]
    pub const fn part(&self) -> ConversationPart {
        match self {
            Self::Missing { part, .. } | Self::Unreadable { part, .. } => *part,
        }
    }

    /// Where the part is, or was expected.
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
    /// A stable identifier for the part, for machine-readable output.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::BaseConfig => "base_config",
            Self::Events => "events",
        }
    }

    /// The file the filesystem backend stores the part in.
    pub(crate) const fn file_name(self) -> &'static str {
        match self {
            Self::Metadata => METADATA_FILE,
            Self::BaseConfig => BASE_CONFIG_FILE,
            Self::Events => EVENTS_FILE,
        }
    }
}

/// Why a part of a stored conversation that exists could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    /// The backend could not hand the part over, e.g. for lack of permission.
    #[error("{0}")]
    Unavailable(String),

    /// The part was read, and does not have the shape the part has.
    #[error("{}{}", malformed(*format), at(*position))]
    Malformed {
        /// The format the backend stores the part in, such as `JSON`, as a
        /// message names it.
        format: Option<&'static str>,

        message: String,

        /// Where in the part's text the problem is, when it is text.
        position: Option<Position>,
    },
}

/// A place in a text, counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

/// `invalid JSON` for a part stored as JSON, `malformed` without a format.
fn malformed(format: Option<&str>) -> String {
    format.map_or_else(
        || "malformed".to_owned(),
        |format| format!("invalid {format}"),
    )
}

/// ` at line 1, column 2`, or nothing without a position.
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
    /// The conversation directory that failed validation.
    #[must_use]
    pub fn path(&self) -> Utf8PathBuf {
        self.conversations_dir.join(&self.dirname)
    }

    /// Where the conversation is stored.
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

    /// Check every conversation directory as [`Self::validate_conversations`]
    /// does, and find the leftovers of interrupted writes, changing nothing on
    /// disk.
    ///
    /// The leftovers are the ones [`Self::validate_conversations`] removes, and
    /// are listed instead.
    /// The scan cache is read, so a directory that passed with the same file
    /// stamps is not read again, and never written.
    pub(crate) fn diagnose_conversations(&self) -> ConversationsDiagnosis {
        trace!("Diagnosing conversations.");
        self.load_scan_cache();

        let mut validation = ValidationResult::default();
        let mut leftovers = vec![];
        for root in [Some(&self.root), self.user.as_ref()].into_iter().flatten() {
            let conversations_dir = root.join(CONVERSATIONS_DIR);
            if !conversations_dir.is_dir() {
                continue;
            }

            leftovers.extend(leftovers_in(&conversations_dir));
            validate_root(&conversations_dir, &self.scan, &mut validation, &mut vec![]);
        }

        let mut invalid = validation.invalid;
        invalid.sort_by_key(InvalidConversation::path);
        let mut unrecognized = validation.unrecognized;
        unrecognized.sort();
        leftovers.sort();

        ConversationsDiagnosis {
            invalid,
            unrecognized,
            leftovers,
        }
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

/// The import staging directories, and the `.tmp` files in conversation
/// directories, under `conversations_dir`: what the startup scan removes.
///
/// Entries whose name holds no conversation ID are not JP's, and are left out.
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

/// Check one conversation directory's files the way startup validation checks
/// each directory under `conversations/`.
///
/// The directory can live anywhere and be named anything.
/// Reads the directory's files and changes nothing on disk.
///
/// # Errors
///
/// Returns the reason the directory fails validation.
pub fn validate_conversation_dir(dir: &Utf8Path) -> Result<(), ValidationError> {
    check_files(dir, &ScanCache::default()).map(|_| ())
}

/// Whether the conversation in `dir` is missing nothing but its metadata: the
/// metadata file is absent, and the rest passes validation.
pub(crate) fn missing_only_metadata(dir: &Utf8Path) -> bool {
    !dir.join(METADATA_FILE).exists() && check_contents(dir).is_ok()
}

/// Write the metadata of conversation `id`, stored in `dir`, from what the rest
/// of the directory holds.
///
/// The title is the one the directory name carries, with each dash read as a
/// space, since the name replaced the title's spaces and punctuation with
/// dashes.
/// The last activity is the time of the last event, or the conversation's
/// creation when it has none.
/// Every other field takes its default.
/// Meant for a directory [`missing_only_metadata`] accepts: an existing
/// `metadata.json` is overwritten.
///
/// # Errors
///
/// Fails when the metadata cannot be written.
pub(crate) fn rebuild_metadata(dir: &Utf8Path, id: &ConversationId) -> crate::error::Result<()> {
    let last_event_at = validate_events(&dir.join(EVENTS_FILE))
        .ok()
        .and_then(|summary| summary.last_event_at);

    let metadata = Conversation {
        title: title_from_dirname(dir, id),
        last_activated_at: last_event_at.unwrap_or_else(|| id.timestamp()),
        ..Conversation::default()
    };

    write_json(&dir.join(METADATA_FILE), &metadata)
}

/// The title the name of `dir` carries after conversation `id`'s timestamp,
/// with dashes read as spaces.
fn title_from_dirname(dir: &Utf8Path, id: &ConversationId) -> Option<String> {
    let slug = dir
        .file_name()?
        .strip_prefix(&id.to_dirname(None))?
        .strip_prefix('-')?;

    let title = slug.replace('-', " ");
    (!title.trim().is_empty()).then_some(title)
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

/// Confirm `metadata.json` is a valid JSON object.
///
/// Uses [`IgnoredAny`] to skip all field values — no allocations, no schema
/// checks.
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
fn validate_events(path: &Utf8Path) -> Result<EventsSummary, ReadError> {
    let buf = fs::read(path)?;
    Ok(EventsSummary::parse(&buf)?)
}

#[cfg(test)]
#[path = "validate_tests.rs"]
mod tests;
