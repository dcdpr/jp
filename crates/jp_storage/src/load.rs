use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, BufReader},
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream, StreamError};
use rayon::iter::{IntoParallelRefIterator as _, ParallelIterator as _};
use serde::{
    Deserialize, Deserializer,
    de::{DeserializeOwned, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use tracing::warn;

use crate::{
    BASE_CONFIG_FILE, CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE, Storage,
    build_conversation_dir_prefix, dir_entries, find_conversation_dir_path,
    load_conversation_id_from_entry, load_inflight_conversation_id, parse_datetime,
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
    /// Load all conversation ids.
    #[must_use]
    pub fn load_all_conversation_ids(&self) -> Vec<ConversationId> {
        let mut conversations = vec![];
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let path = root.join(CONVERSATIONS_DIR);
            conversations.extend(scan_conversation_ids(&path));
        }

        conversations.sort();
        conversations
    }
}

/// Scan a single conversations directory for IDs.
///
/// If any in-flight persist directories (`.old-*`, `.staging-*`) are found
/// without a corresponding normal directory, retries briefly to let the atomic
/// rename complete.
/// This ensures every returned ID has a normal directory behind it, even when
/// another process is mid-persist.
fn scan_conversation_ids(path: &Utf8Path) -> Vec<ConversationId> {
    let entries: Vec<_> = dir_entries(path).collect();

    let normal: HashSet<_> = entries
        .par_iter()
        .filter_map(load_conversation_id_from_entry)
        .collect();

    let missing_inflight: Vec<_> = entries
        .par_iter()
        .filter_map(load_inflight_conversation_id)
        .filter(|id| !normal.contains(id))
        .collect();

    if missing_inflight.is_empty() {
        return normal.into_iter().collect();
    }

    // Another process is mid-atomic-swap. Retry briefly — the rename gap
    // is nanoseconds, so 10 × 1ms is extremely generous.
    let mut ids: HashSet<_> = normal;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(1));

        let found: Vec<_> = dir_entries(path)
            .filter_map(|e| load_conversation_id_from_entry(&e))
            .filter(|id| missing_inflight.contains(id) && !ids.contains(id))
            .collect();

        ids.extend(found.iter().copied());

        if missing_inflight.iter().all(|id| ids.contains(id)) {
            break;
        }
    }

    ids.into_iter().collect()
}

impl Storage {
    pub fn load_conversation_stream(&self, id: &ConversationId) -> Result<ConversationStream> {
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let Some(conv_dir) = find_conversation_dir_path(root, id) else {
                continue;
            };

            let events_path = conv_dir.join(EVENTS_FILE);
            if !events_path.is_file() {
                continue;
            }

            let base_config_path = conv_dir.join(BASE_CONFIG_FILE);
            if base_config_path.is_file() {
                // New format: separate `base_config.json` and `events.json`.
                let base_config = load_json(&base_config_path)?;
                let events = load_json(&events_path)?;

                return ConversationStream::from_parts(base_config, events)
                    .map(|stream| stream.with_created_at(id.timestamp()))
                    .map_err(|error| LoadError {
                        path: conv_dir,
                        error: LoadErrorInner::Stream(error),
                    });
            }

            // Legacy format: base config packed as first element in events.json.
            let events = load_json(&events_path)?;
            match ConversationStream::from_legacy_events(events) {
                Ok(Some(stream)) => return Ok(stream),
                Ok(None) => {
                    return Err(LoadError {
                        path: conv_dir,
                        error: LoadErrorInner::Stream(StreamError::FromEmptyIterator),
                    });
                }
                Err(error) => {
                    return Err(LoadError {
                        path: conv_dir,
                        error: LoadErrorInner::Stream(error),
                    });
                }
            }
        }

        let path = build_conversation_dir_prefix(&self.root, id);

        Err(LoadError {
            path,
            error: LoadErrorInner::MissingConversationStream(*id),
        })
    }

    pub fn load_conversation_metadata(&self, id: &ConversationId) -> Result<Conversation> {
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let Some(conv_dir) = find_conversation_dir_path(root, id) else {
                continue;
            };

            let is_user = Some(root) == self.user.as_ref();
            match Self::load_conversation_metadata_at(&conv_dir, id, is_user) {
                Err(error) if error.kind().is_missing() => {}
                other => return other,
            }
        }

        Err(LoadError {
            path: build_conversation_dir_prefix(&self.root, id),
            error: LoadErrorInner::MissingConversationMetadata(*id),
        })
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
        let mut dirs: HashMap<ConversationId, (Utf8PathBuf, bool)> = HashMap::new();
        for (root, is_user) in [(Some(&self.root), false), (self.user.as_ref(), true)] {
            let Some(root) = root else {
                continue;
            };
            for entry in dir_entries(root.join(CONVERSATIONS_DIR)) {
                if let Some(id) = load_conversation_id_from_entry(&entry) {
                    // The root (non-user) directory wins, matching the root
                    // precedence in `load_conversation_metadata`.
                    dirs.entry(id)
                        .or_insert_with(|| (entry.into_path(), is_user));
                }
            }
        }

        ids.par_iter()
            .map(|id| {
                // Fast path: read straight from the resolved directory. On any
                // miss or failure (stale path, corrupt active metadata), fall
                // back to the rescanning loader, including the archive — this
                // mirrors `FsStorageBackend::load_conversation_metadata`.
                let result = match dirs.get(id) {
                    Some((dir, is_user)) => Self::load_conversation_metadata_at(dir, id, *is_user),
                    None => Err(LoadError {
                        path: build_conversation_dir_prefix(&self.root, id),
                        error: LoadErrorInner::MissingConversationMetadata(*id),
                    }),
                }
                .or_else(|_| self.load_conversation_metadata(id))
                .or_else(|_| self.load_archived_conversation_metadata(id));

                (*id, result)
            })
            .collect()
    }

    /// Load conversation metadata from an already-resolved directory.
    ///
    /// Returns a [`LoadErrorInner::MissingConversationMetadata`] error when the
    /// directory holds no `metadata.json`, so callers can fall through to the
    /// next storage root.
    fn load_conversation_metadata_at(
        conv_dir: &Utf8Path,
        id: &ConversationId,
        is_user: bool,
    ) -> Result<Conversation> {
        let path = conv_dir.join(METADATA_FILE);
        if !path.is_file() {
            return Err(LoadError {
                path,
                error: LoadErrorInner::MissingConversationMetadata(*id),
            });
        }

        let mut conversation: Conversation = load_json(&path)?;
        conversation.user = is_user;
        (conversation.events_count, conversation.last_event_at) =
            load_count_and_timestamp_events(conv_dir).unwrap_or((0, None));

        Ok(conversation)
    }
}

/// Read and deserialize a JSON file, mapping errors to [`LoadError`].
pub(crate) fn load_json<T: DeserializeOwned>(path: &Utf8Path) -> Result<T> {
    let file = fs::File::open(path).map_err(|error| LoadError {
        path: path.to_path_buf(),
        error: error.into(),
    })?;

    let reader = BufReader::new(file);
    serde_json::from_reader(reader).map_err(|error| LoadError {
        path: path.to_path_buf(),
        error: error.into(),
    })
}

pub(crate) fn load_count_and_timestamp_events(
    root: &Utf8Path,
) -> Option<(usize, Option<DateTime<Utc>>)> {
    let path = root.join(EVENTS_FILE);
    let file = fs::File::open(&path).ok()?;
    let reader = BufReader::new(file);

    let mut deserializer = serde_json::Deserializer::from_reader(reader);
    let summary = match EventSummary::deserialize(&mut deserializer) {
        Ok(summary) => summary,
        Err(error) => {
            warn!(
                error = error.to_string(),
                path = path.as_str(),
                "Error parsing JSON event file."
            );
            return None;
        }
    };

    let last_timestamp = summary.last_timestamp.and_then(|ts| {
        let ts = ts.get();
        if ts.len() >= 2 && ts.starts_with('"') && ts.ends_with('"') {
            parse_datetime(&ts[1..ts.len() - 1])
        } else {
            None
        }
    });

    Some((summary.count, last_timestamp))
}

/// Streaming summary of a conversation's event array.
///
/// Counts events and keeps only the last event's raw timestamp, pulling one
/// element at a time from the reader so the whole array is never materialized.
struct EventSummary {
    count: usize,
    last_timestamp: Option<Box<RawValue>>,
}

impl<'de> Deserialize<'de> for EventSummary {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SummaryVisitor;

        impl<'de> Visitor<'de> for SummaryVisitor {
            type Value = EventSummary;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an array of conversation events")
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                #[derive(Deserialize)]
                struct RawEvent {
                    timestamp: Box<RawValue>,
                }

                let mut count = 0;
                let mut last_timestamp = None;
                while let Some(event) = seq.next_element::<RawEvent>()? {
                    count += 1;
                    last_timestamp = Some(event.timestamp);
                }

                Ok(EventSummary {
                    count,
                    last_timestamp,
                })
            }
        }

        deserializer.deserialize_seq(SummaryVisitor)
    }
}
