use std::{fs, io::BufReader};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Utc};
use jp_conversation::{Conversation, ConversationId, ConversationStream, ConversationsMetadata};
use rayon::iter::{IntoParallelRefIterator as _, ParallelIterator as _};
use relative_path::RelativePath;
use serde::de::DeserializeOwned;
use tracing::{trace, warn};

use crate::{
    CONVERSATIONS_DIR, EVENTS_FILE, METADATA_FILE, Storage, build_conversation_dir_prefix,
    dir_entries, find_conversation_dir_path, load_conversation_id_from_entry, parse_datetime,
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
    /// Gets the underlying [`LoadErrorInner`] that provides more details on
    /// what went wrong.
    #[must_use]
    pub fn kind(&self) -> &LoadErrorInner {
        &self.error
    }

    /// Returns `true` if the error is caused by corrupt or invalid data
    /// (as opposed to a system-level I/O failure or missing data).
    #[must_use]
    pub fn is_corrupt(&self) -> bool {
        matches!(self.error, LoadErrorInner::Json(_))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadErrorInner {
    #[error(transparent)]
    IO(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

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
    /// Loads the conversations metadata from storage.
    ///
    /// This loads the file from user storage if configured, otherwise the
    /// workspace storage is used.
    ///
    /// If the file does not exist, return default conversations metadata.
    pub fn load_conversations_metadata(&self) -> Result<ConversationsMetadata> {
        let metadata_path = RelativePath::new(CONVERSATIONS_DIR)
            .to_owned()
            .join(METADATA_FILE);

        trace!(path = %metadata_path, "Loading user conversations metadata.");

        let path = self.user_or_root_with_path(&metadata_path);

        if !path.exists() {
            return Ok(ConversationsMetadata::default());
        }

        self.read_json(&path)
    }

    pub fn load_all_conversation_ids(&self) -> Vec<ConversationId> {
        let mut conversations = vec![];
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let path = root.join(CONVERSATIONS_DIR);
            conversations.extend(
                dir_entries(&path)
                    .collect::<Vec<_>>()
                    .par_iter()
                    .filter_map(load_conversation_id_from_entry)
                    .collect::<Vec<_>>(),
            );
        }

        conversations.sort();
        conversations
    }

    pub fn load_conversation_stream(&self, id: &ConversationId) -> Result<ConversationStream> {
        let this = &self;
        for root in [Some(&this.root), this.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let Some(path) = find_conversation_dir_path(root, id).map(|v| v.join(EVENTS_FILE))
            else {
                continue;
            };

            if !path.is_file() {
                continue;
            }

            return self.read_json(&path);
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

            let path = conv_dir.join(METADATA_FILE);
            if !path.is_file() {
                continue;
            }

            let mut conversation: Conversation = self.read_json(&path)?;
            conversation.user = Some(root) == self.user.as_ref();
            (conversation.events_count, conversation.last_event_at) =
                load_count_and_timestamp_events(&conv_dir).unwrap_or((0, None));

            return Ok(conversation);
        }

        Err(LoadError {
            path: build_conversation_dir_prefix(&self.root, id),
            error: LoadErrorInner::MissingConversationMetadata(*id),
        })
    }

    // #[must_use]
    // pub fn load_all_conversations_details(&self) -> HashMap<ConversationId, Conversation> {
    //     let mut conversations = HashMap::new();
    //     for root in [Some(&self.root), self.user.as_ref()] {
    //         let Some(root) = root else {
    //             continue;
    //         };
    //
    //         let path = root.join(CONVERSATIONS_DIR);
    //         let details = dir_entries(&path)
    //             .collect::<Vec<_>>()
    //             .into_par_iter()
    //             .filter_map(|entry| {
    //                 let (id, mut conversation) = load_conversation_metadata(&entry)?;
    //                 conversation.user = Some(root) == self.user.as_ref();
    //                 (conversation.events_count, conversation.last_event_at) =
    //                     load_count_and_timestamp_events(&entry.path()).unwrap_or((0, None));
    //
    //                 Some((id, conversation))
    //             })
    //             .collect::<Vec<_>>();
    //
    //         conversations.extend(details);
    //     }
    //     conversations
    // }
    //
    //

    // FIXME: This can't be relative since we sometimes need to read JSON from
    // the workspace or user storage. Optionally we split the storage types
    // between workspace and user, and have dedicated read_json methods, but
    // that seems perhaps a bit overkill?
    pub fn read_json<T: DeserializeOwned>(&self, path: &Utf8Path) -> Result<T> {
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
}

fn load_count_and_timestamp_events(root: &Utf8Path) -> Option<(usize, Option<DateTime<Utc>>)> {
    #[derive(serde::Deserialize)]
    struct RawEvent {
        timestamp: Box<serde_json::value::RawValue>,
    }
    let path = root.join(EVENTS_FILE);
    let file = fs::File::open(&path).ok()?;
    let reader = BufReader::new(file);

    let events: Vec<RawEvent> = match serde_json::from_reader(reader) {
        Ok(events) => events,
        Err(error) => {
            warn!(
                error = error.to_string(),
                path = path.as_str(),
                "Error parsing JSON event file."
            );
            return None;
        }
    };

    let mut event_count = 0;
    let mut last_timestamp = None;
    for event in events {
        event_count += 1;
        let ts = event.timestamp.get();
        if ts.len() >= 2 && ts.starts_with('"') && ts.ends_with('"') {
            last_timestamp = parse_datetime(&ts[1..ts.len() - 1]);
        }
    }

    Some((event_count, last_timestamp))
}
