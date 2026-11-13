//! Cached scan results for conversation directories.
//!
//! Validating a conversation directory and counting the events in its
//! `events.json` both parse the whole events file, which dominates startup in a
//! workspace with many conversations.
//! [`ScanCache`] records each directory that passed validation, together with
//! the size and modification time of every file the check read and the
//! [`EventsSummary`] computed from the same parse.
//! Later scans, in the current process or a subsequent process, skip parsing
//! any directory whose files still match the recorded [`FileStamp`]s.
//! Directories with a file modified within [`MTIME_GRANULARITY`] of the scan
//! are not cached, because a rewrite in the same timestamp tick could leave the
//! stamps unchanged.
//!
//! Across processes, the cache is stored at [`SCAN_CACHE_FILE`] in the
//! user-local `conversations/` directory, which is outside any git checkout.
//! Cached entries are used only when their stamps match the files on disk, so a
//! missing, stale, or unreadable cache file only triggers a re-parse.

use std::{
    collections::{BTreeMap, HashMap},
    fmt, fs,
    sync::{Arc, PoisonError, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{IgnoredAny, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use tracing::{debug, trace};

use crate::{BASE_CONFIG_FILE, EVENTS_FILE, METADATA_FILE, parse_datetime};

/// File name of the persisted cache inside the user-local `conversations/`
/// directory.
///
/// Uses a leading dot so conversation lookups and validation skip it.
pub(crate) const SCAN_CACHE_FILE: &str = ".scan_cache.json";

/// Schema version for the persisted scan cache and its validation rules.
///
/// Increment this when the structural checks in `validate` or the
/// [`EventsSummary`] computation change, so entries written by older versions
/// are discarded.
const SCAN_CACHE_VERSION: u32 = 1;

/// Minimum age of a file's modification time relative to the start of a scan
/// for the scan result to be cached.
///
/// Filesystems with coarse timestamp resolution (2 s on FAT, 1 s on HFS+) can
/// assign the same modification time when a file is rewritten within the same
/// clock tick as the scan.
/// Once a file's modification time is older than this window, any later write
/// produces a newer timestamp.
const MTIME_GRANULARITY: Duration = Duration::from_secs(2);

/// Size and modification time of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileStamp {
    size: u64,
    modified_ns: u64,
}

impl FileStamp {
    /// Read the size and modification time of the regular file at `path`.
    ///
    /// Returns `None` if the path does not exist, is not a regular file, or has
    /// no modification time.
    pub(crate) fn of(path: &Utf8Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }

        let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
        Some(Self {
            size: meta.len(),
            modified_ns: u64::try_from(modified.as_nanos()).ok()?,
        })
    }

    fn modified_before(self, cutoff: SystemTime) -> bool {
        UNIX_EPOCH + Duration::from_nanos(self.modified_ns) < cutoff
    }
}

/// Event count and last-event timestamp for a conversation's `events.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EventsSummary {
    pub(crate) count: usize,
    pub(crate) last_event_at: Option<DateTime<Utc>>,
}

impl EventsSummary {
    /// Parse an `events.json` body.
    ///
    /// Fails unless the body is a JSON array whose elements are all objects
    /// with `timestamp` and `type` fields.
    /// Field values are skipped without being materialized; only the last
    /// element's timestamp is parsed, and an unparseable last timestamp yields
    /// `last_event_at: None` rather than an error.
    pub(crate) fn parse(bytes: &[u8]) -> serde_json::Result<Self> {
        let raw: RawSummary<'_> = serde_json::from_slice(bytes)?;
        let last_event_at = raw.last_timestamp.and_then(|ts| {
            let ts = ts.get();
            ts.strip_prefix('"')
                .and_then(|ts| ts.strip_suffix('"'))
                .and_then(parse_datetime)
        });

        Ok(Self {
            count: raw.count,
            last_event_at,
        })
    }
}

/// Element count and borrowed last timestamp of an events array.
struct RawSummary<'a> {
    count: usize,
    last_timestamp: Option<&'a RawValue>,
}

impl<'de> Deserialize<'de> for RawSummary<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SummaryVisitor;

        impl<'de> Visitor<'de> for SummaryVisitor {
            type Value = RawSummary<'de>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an array of conversation events")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                // Config deltas and events both serialize `timestamp` and
                // `type` at the top level.
                #[derive(Deserialize)]
                struct EventProbe<'a> {
                    #[serde(borrow)]
                    timestamp: &'a RawValue,
                    #[expect(dead_code)]
                    r#type: IgnoredAny,
                }

                let mut count = 0;
                let mut last_timestamp = None;
                while let Some(event) = seq.next_element::<EventProbe<'de>>()? {
                    count += 1;
                    last_timestamp = Some(event.timestamp);
                }

                Ok(RawSummary {
                    count,
                    last_timestamp,
                })
            }
        }

        deserializer.deserialize_seq(SummaryVisitor)
    }
}

/// File stamps for the files in a conversation directory, captured before the
/// files are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirStamps {
    pub(crate) metadata: Option<FileStamp>,
    pub(crate) base_config: Option<FileStamp>,
    pub(crate) events: Option<FileStamp>,
}

impl DirStamps {
    pub(crate) fn of(dir: &Utf8Path) -> Self {
        Self {
            metadata: FileStamp::of(&dir.join(METADATA_FILE)),
            base_config: FileStamp::of(&dir.join(BASE_CONFIG_FILE)),
            events: FileStamp::of(&dir.join(EVENTS_FILE)),
        }
    }

    /// Build a [`ScannedDir`] for a directory that passed validation with these
    /// stamps.
    ///
    /// Returns `None` if `metadata.json` or `events.json` is missing a stamp.
    pub(crate) fn scanned(self, summary: EventsSummary) -> Option<ScannedDir> {
        Some(ScannedDir {
            metadata: self.metadata?,
            base_config: self.base_config,
            events: self.events?,
            summary,
        })
    }
}

/// A conversation directory that passed validation, with the stamps of the
/// files that were checked and the summary of its events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScannedDir {
    metadata: FileStamp,
    base_config: Option<FileStamp>,
    events: FileStamp,
    summary: EventsSummary,
}

impl ScannedDir {
    fn matches(&self, stamps: &DirStamps) -> bool {
        stamps.metadata == Some(self.metadata)
            && stamps.base_config == self.base_config
            && stamps.events == Some(self.events)
    }

    /// Returns `true` if every file was modified more than
    /// [`MTIME_GRANULARITY`] before `scan_started`, making the scan safe to
    /// cache.
    fn reusable(&self, scan_started: SystemTime) -> bool {
        let cutoff = scan_started
            .checked_sub(MTIME_GRANULARITY)
            .unwrap_or(UNIX_EPOCH);

        [Some(self.metadata), self.base_config, Some(self.events)]
            .into_iter()
            .flatten()
            .all(|stamp| stamp.modified_before(cutoff))
    }
}

/// On-disk form of the cache.
#[derive(Serialize, Deserialize)]
struct Persisted {
    version: u32,
    dirs: BTreeMap<Utf8PathBuf, ScannedDir>,
}

/// Scanned conversation directories, keyed by directory path.
///
/// Clones share one set of entries.
#[derive(Clone, Default)]
pub(crate) struct ScanCache {
    state: Arc<RwLock<State>>,
}

#[derive(Default)]
struct State {
    /// Cached directories from both the current process and the on-disk cache.
    dirs: HashMap<Utf8PathBuf, ScannedDir>,

    /// The entries last read from or written to disk, used to skip rewriting an
    /// unchanged cache file.
    on_disk: BTreeMap<Utf8PathBuf, ScannedDir>,
}

impl fmt::Debug for ScanCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("ScanCache")
            .field("dirs", &state.dirs.len())
            .finish()
    }
}

impl ScanCache {
    /// Return the cached scan for `dir` if all of its file stamps match
    /// `stamps`.
    pub(crate) fn get(&self, dir: &Utf8Path, stamps: &DirStamps) -> Option<ScannedDir> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state
            .dirs
            .get(dir)
            .filter(|scanned| scanned.matches(stamps))
            .copied()
    }

    /// Return the cached [`EventsSummary`] for `dir` if its `events.json` stamp
    /// matches `events`.
    pub(crate) fn summary(&self, dir: &Utf8Path, events: FileStamp) -> Option<EventsSummary> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state
            .dirs
            .get(dir)
            .filter(|scanned| scanned.events == events)
            .map(|scanned| scanned.summary)
    }

    /// Record directories that passed a scan started at `scan_started`.
    ///
    /// A directory with a file modified within [`MTIME_GRANULARITY`] of
    /// `scan_started` is not recorded.
    pub(crate) fn extend(
        &self,
        scanned: impl IntoIterator<Item = (Utf8PathBuf, ScannedDir)>,
        scan_started: SystemTime,
    ) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.dirs.extend(
            scanned
                .into_iter()
                .filter(|(_, scanned)| scanned.reusable(scan_started)),
        );
    }

    /// Load persisted cache entries from `path` into memory.
    ///
    /// Missing, unreadable, or version-mismatched cache files are ignored.
    pub(crate) fn load(&self, path: &Utf8Path) {
        let Ok(bytes) = fs::read(path) else {
            return;
        };

        let persisted = match serde_json::from_slice::<Persisted>(&bytes) {
            Ok(persisted) if persisted.version == SCAN_CACHE_VERSION => persisted,
            Ok(persisted) => {
                debug!(%path, version = persisted.version, "Ignoring scan cache of another version.");
                return;
            }
            Err(error) => {
                debug!(%path, %error, "Ignoring unreadable scan cache.");
                return;
            }
        };

        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.dirs.extend(
            persisted
                .dirs
                .iter()
                .map(|(dir, scanned)| (dir.clone(), *scanned)),
        );
        state.on_disk = persisted.dirs;
    }

    /// Persist `scanned` to `path`, replacing the existing cache file.
    ///
    /// Only entries with an absolute directory path whose files were all
    /// modified more than [`MTIME_GRANULARITY`] before `scan_started` are
    /// written.
    /// Skips the write if the filtered entry set matches the on-disk state, and
    /// logs and ignores I/O errors.
    pub(crate) fn save(
        &self,
        path: &Utf8Path,
        scanned: impl IntoIterator<Item = (Utf8PathBuf, ScannedDir)>,
        scan_started: SystemTime,
    ) {
        let dirs: BTreeMap<_, _> = scanned
            .into_iter()
            .filter(|(dir, scanned)| dir.is_absolute() && scanned.reusable(scan_started))
            .collect();

        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        if state.on_disk == dirs {
            return;
        }

        let persisted = Persisted {
            version: SCAN_CACHE_VERSION,
            dirs,
        };
        if let Err(error) = write_atomic(path, &persisted) {
            debug!(%path, %error, "Failed to write scan cache.");
            return;
        }

        trace!(%path, count = persisted.dirs.len(), "Wrote scan cache.");
        state.on_disk = persisted.dirs;
    }
}

/// Write `value` to `path` through a temporary sibling unique to this call.
///
/// Concurrent processes save the same cache file, so the temporary name must
/// not collide the way a fixed `{path}.tmp` would.
fn write_atomic(path: &Utf8Path, value: &impl Serialize) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = Utf8PathBuf::from(format!("{path}.{}-{nanos}.tmp", std::process::id()));

    let result = fs::write(&tmp, bytes).and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _err = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
