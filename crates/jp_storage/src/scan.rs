//! Cached scan results for conversation directories.
//!
//! Validating a conversation directory and counting the events in its
//! `events.json` both parse the whole events file, which dominates startup in a
//! workspace with many conversations.
//! [`ScanCache`] records each directory that passed validation, together with
//! the size and modification time of every file the check read and the
//! [`EventsSummary`] computed from the same parse.
//! A later scan, in this process or a later one, skips the parse for a
//! directory whose files still carry the recorded [`FileStamp`]s.
//! A directory with a file modified within [`MTIME_GRANULARITY`] of the scan is
//! never recorded, because its stamps could still match after a rewrite.
//!
//! Between processes the cache lives in [`SCAN_CACHE_FILE`] inside the
//! user-local `conversations/` directory, which is never part of a git
//! checkout.
//! An entry is only trusted after its stamps match the files on disk, so a
//! missing, stale, or unreadable cache file costs a re-parse and nothing else.

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
/// Dot-prefixed so conversation lookups and validation skip it.
pub(crate) const SCAN_CACHE_FILE: &str = ".scan_cache.json";

/// Version of the persisted format, and of the checks an entry vouches for.
///
/// Bump it when the structural checks in `validate` or the [`EventsSummary`]
/// computation change, so entries recorded under the old rules are discarded.
const SCAN_CACHE_VERSION: u32 = 1;

/// How long a file's modification time must lie before the scan started for the
/// scan of it to be recorded.
///
/// Filesystems with coarse timestamps (FAT records 2 s, HFS+ 1 s) can give a
/// file rewritten within one tick the modification time it had when it was
/// scanned.
/// Once a file's modification time is older than this window, any later rewrite
/// carries a newer one, so the stamp cannot match stale content.
const MTIME_GRANULARITY: Duration = Duration::from_secs(2);

/// Size and modification time of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileStamp {
    size: u64,
    modified_ns: u64,
}

impl FileStamp {
    /// Stamp the regular file at `path`.
    ///
    /// Returns `None` when the path is missing, is not a regular file, or the
    /// platform reports no modification time for it.
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

/// Number of events in a conversation's `events.json`, and the timestamp of the
/// last one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EventsSummary {
    pub(crate) count: usize,
    pub(crate) last_event_at: Option<DateTime<Utc>>,
}

impl EventsSummary {
    /// Parse an `events.json` body.
    ///
    /// Fails unless the body is a JSON array whose elements are all objects
    /// carrying `timestamp` and `type` fields.
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

/// Stamps of the files validation reads in one conversation directory, taken
/// before any of them is read.
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

    /// Record a directory that passed validation with these stamps.
    ///
    /// Returns `None` when `metadata.json` or `events.json` could not be
    /// stamped, since such a directory cannot be recognized again later.
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

    /// Whether a scan started at `scan_started` may be recorded: every file was
    /// last modified more than [`MTIME_GRANULARITY`] before it.
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
    /// Every recorded directory: those scanned by this process and those read
    /// from disk.
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
    /// The recorded scan of `dir`, if every file still carries its recorded
    /// stamp.
    pub(crate) fn get(&self, dir: &Utf8Path, stamps: &DirStamps) -> Option<ScannedDir> {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state
            .dirs
            .get(dir)
            .filter(|scanned| scanned.matches(stamps))
            .copied()
    }

    /// The events summary recorded for `dir`, if its `events.json` still
    /// carries the recorded stamp.
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

    /// Merge in the entries persisted at `path`.
    ///
    /// A missing file, unreadable file, or file written under another
    /// [`SCAN_CACHE_VERSION`] adds nothing.
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

    /// Persist `scanned` to `path`, replacing what the file held.
    ///
    /// Only entries with an absolute directory path whose files were all
    /// modified at least [`MTIME_GRANULARITY`] before `scan_started` are
    /// written.
    /// The file is left untouched when that set equals what was last read from
    /// or written to it.
    /// Failures are logged and otherwise ignored: the cache only saves work.
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
