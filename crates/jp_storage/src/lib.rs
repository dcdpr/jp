pub mod error;
pub mod value;

use std::{
    fs,
    io::BufReader,
    iter,
    path::{Path, PathBuf},
};

use ahash::{HashMap, HashMapExt};
pub use error::Error;
use jp_conversation::{Conversation, ConversationId, ConversationStream, ConversationsMetadata};
use jp_id::Id as _;
use jp_tombmap::TombMap;
use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};
use time::{UtcDateTime, macros::format_description};
use tracing::{trace, warn};

use crate::{
    error::Result,
    value::{read_json, write_json},
};

pub const DEFAULT_STORAGE_DIR: &str = ".jp";
pub const METADATA_FILE: &str = "metadata.json";
const EVENTS_FILE: &str = "events.json";
pub const CONVERSATIONS_DIR: &str = "conversations";

#[derive(Debug)]
pub struct Storage {
    /// The path to the original storage directory.
    root: PathBuf,

    /// The path to the user storage directory.
    ///
    /// This is used (among other things) to store the active conversation id
    /// that are tied to the current user.
    ///
    /// If unset, user storage is disabled.
    user: Option<PathBuf>,
}

impl Storage {
    /// Creates a new Storage instance by creating a temporary directory and
    /// copying the contents of `root` into it.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        // Create root storage directory, if needed.
        let root: PathBuf = root.into();
        if root.exists() {
            if !root.is_dir() {
                return Err(Error::NotDir(root));
            }
        } else {
            fs::create_dir_all(&root)?;
            trace!(path = %root.display(), "Created storage directory.");
        }

        Ok(Self { root, user: None })
    }

    pub fn with_user_storage(
        mut self,
        root: &Path,
        name: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self> {
        let name: String = name.into();
        let id: String = id.into();
        let dirname = format!("{name}-{id}");
        let mut path = root.join(&dirname);

        // Create user storage directory, if needed.
        if root.exists()
            && let Some(mut existing_path) = fs::read_dir(root)?.find_map(|entry| {
                let path = entry.ok()?.path();
                path.to_string_lossy().ends_with(&id).then_some(path)
            })
        {
            if !existing_path.is_dir() {
                return Err(Error::NotDir(existing_path));
            }

            // At this point we know we have a user workspace directory ending
            // with the correct ID, but it might not have the correct name. This
            // can happen if the user has renamed the workspace directory.
            if let Some(suffix) = existing_path.file_name()
                && suffix != dirname.as_str()
            {
                let new_path = existing_path.with_file_name(dirname);
                trace!(
                    old = %existing_path.display(),
                    new = %new_path.display(),
                    "Renaming existing user storage directory to match new name."
                );
                fs::rename(&existing_path, &new_path)?;
                existing_path = new_path;
            }

            // If the symlink already exists, but points to a different instance
            // of the workspace, remove the symlink, so we can re-link to the
            // current workspace instance.
            if let Some(existing) = existing_path
                .join("storage")
                .read_link()
                .ok()
                .filter(|v| v != &self.root)
            {
                trace!(existing = %existing.display(), "Removing existing user storage symlink.");
                fs::remove_file(existing_path.join("storage"))?;
            }

            path = existing_path;
        } else {
            fs::create_dir_all(&path)?;
            trace!(path = %path.display(), "Created user storage directory.");
        }

        // Create reference back to workspace storage.
        let link = path.join("storage");
        if link.exists() {
            if !link.is_symlink() {
                return Err(Error::NotSymlink(link));
            }
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&self.root, path.join("storage"))?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&self.root, path.join("storage"))?;
            #[cfg(not(any(unix, windows)))]
            {
                tracing::error!(
                    "Unsupported platform, cannot create symlink. Disabling user storage."
                );
                return Ok(self);
            }
        }

        self.user = Some(path);
        Ok(self)
    }

    /// Returns the path to the storage directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Returns the path to the user storage directory, if configured.
    #[must_use]
    pub fn user_storage_path(&self) -> Option<&Path> {
        self.user.as_deref()
    }

    /// Loads the conversations metadata from storage.
    ///
    /// This loads the file from user storage if configured, otherwise the
    /// workspace storage is used.
    ///
    /// If the file does not exist, return default conversations metadata.
    pub fn load_conversations_metadata(&self) -> Result<ConversationsMetadata> {
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        trace!(path = %metadata_path.display(), "Loading user conversations metadata.");

        if !metadata_path.exists() {
            return Ok(ConversationsMetadata::default());
        }

        read_json(&metadata_path)
    }

    pub fn load_conversation_metadata(&self, id: &ConversationId) -> Result<Conversation> {
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let Some(path) = find_conversation_dir_path(root, id).map(|v| v.join(METADATA_FILE))
            else {
                continue;
            };

            if path.is_file() {
                return read_json(&path);
            }
        }

        Err(jp_conversation::Error::UnknownId(*id).into())
    }

    #[must_use]
    pub fn load_all_conversations_details(&self) -> HashMap<ConversationId, Conversation> {
        let mut conversations = HashMap::new();
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let path = root.join(CONVERSATIONS_DIR);
            let details = dir_entries(&path)
                .collect::<Vec<_>>()
                .into_par_iter()
                .filter_map(|entry| {
                    let (id, mut conversation) = load_conversation_metadata(&entry)?;
                    conversation.user = Some(root) == self.user.as_ref();
                    (conversation.events_count, conversation.last_event_at) =
                        load_count_and_timestamp_events(&entry).unwrap_or((0, None));

                    Some((id, conversation))
                })
                .collect::<Vec<_>>();

            conversations.extend(details);
        }
        conversations
    }

    pub fn load_conversation_events(&self, id: &ConversationId) -> Result<ConversationStream> {
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let Some(path) = find_conversation_dir_path(root, id).map(|v| v.join(EVENTS_FILE))
            else {
                continue;
            };

            if path.is_file() {
                return read_json(&path);
            }
        }

        Err(jp_conversation::Error::UnknownId(*id).into())
    }

    pub fn persist_conversations_and_events(
        &mut self,
        conversations: &TombMap<ConversationId, Conversation>,
        events: &TombMap<ConversationId, ConversationStream>,
        active_conversation_id: &ConversationId,
        active_conversation: &Conversation,
    ) -> Result<()> {
        let root = self.root.as_path();
        let user = self.user.as_deref().unwrap_or(root);

        let conversations_dir = root.join(CONVERSATIONS_DIR);
        let user_conversations_dir = user.join(CONVERSATIONS_DIR);

        trace!(
            global = %conversations_dir.display(),
            user = %user_conversations_dir.display(),
            "Persisting conversations."
        );

        // Append the active conversation to the list of conversations to
        // persist.
        let all_conversations = conversations
            .iter()
            .chain(iter::once((active_conversation_id, active_conversation)));

        for (id, conversation) in all_conversations {
            let dir_name = id.to_dirname(conversation.title.as_deref());
            let conv_dir = if conversation.user {
                user_conversations_dir.join(dir_name)
            } else {
                conversations_dir.join(dir_name)
            };

            // Only remove unused conversations if their IDs have changed.
            if conversations.is_modified(id)
                || conversations.is_removed(id)
                || id == active_conversation_id
            {
                remove_unused_conversation_dirs(
                    id,
                    &conv_dir,
                    &conversations_dir,
                    &user_conversations_dir,
                )?;
            }

            fs::create_dir_all(&conv_dir)?;

            // Write conversation metadata
            let meta_path = conv_dir.join(METADATA_FILE);
            write_json(&meta_path, conversation)?;

            let events_path = conv_dir.join(EVENTS_FILE);
            if let Some(stream) = events.get(id) {
                write_json(&events_path, stream)?;
            }
        }

        // Don't mark active conversation as removed.
        let removed_ids = conversations
            .removed_keys()
            .filter(|&id| id != active_conversation_id)
            .collect::<Vec<_>>();

        for dir in [&conversations_dir, &user_conversations_dir] {
            let mut deleted = Vec::new();
            for entry in dir.read_dir()?.flatten() {
                let path = entry.path();
                let dir_matches_id = path.file_name().is_some_and(|v| {
                    removed_ids.iter().any(|d| {
                        let file_name = v.to_string_lossy();
                        let removed_id = d.target_id();

                        file_name == *removed_id || file_name.starts_with(&format!("{removed_id}-"))
                    })
                });

                if path.is_dir()
                    && dir_matches_id
                    && let Ok(path) = path.strip_prefix(dir)
                {
                    deleted.push(path.to_path_buf());
                }
            }

            remove_deleted(root, dir, deleted.into_iter())?;
        }

        Ok(())
    }

    pub fn persist_conversations_metadata(
        &mut self,
        metadata: &ConversationsMetadata,
    ) -> Result<()> {
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        trace!(path = %metadata_path.display(), "Persisting user conversations metadata.");

        write_json(&metadata_path, metadata)?;

        Ok(())
    }
}

fn load_count_and_timestamp_events(entry: &fs::DirEntry) -> Option<(usize, Option<UtcDateTime>)> {
    #[derive(serde::Deserialize)]
    struct RawEvent {
        timestamp: Box<serde_json::value::RawValue>,
    }
    let fmt = format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond]");
    let path = entry.path().join(EVENTS_FILE);
    let file = fs::File::open(&path).ok()?;
    let reader = BufReader::new(file);

    let events: Vec<RawEvent> = match serde_json::from_reader(reader) {
        Ok(events) => events,
        Err(error) => {
            warn!(%error, path = %path.display(), "Error parsing JSON event file.");
            return None;
        }
    };

    let mut event_count = 0;
    let mut last_timestamp = None;
    for event in events {
        event_count += 1;
        let ts = event.timestamp.get();
        if ts.len() >= 2 && ts.starts_with('"') && ts.ends_with('"') {
            last_timestamp = UtcDateTime::parse(&ts[1..ts.len() - 1], &fmt).ok();
        }
    }

    Some((event_count, last_timestamp))
}

fn dir_entries(path: &Path) -> impl Iterator<Item = fs::DirEntry> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
}

fn find_conversation_dir_path(root: &Path, id: &ConversationId) -> Option<PathBuf> {
    fs::read_dir(root.join(CONVERSATIONS_DIR))
        .ok()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|v| v.starts_with(&id.to_dirname(None)))
        })
        .map(|entry| entry.path())
}

fn load_conversation_metadata(entry: &fs::DirEntry) -> Option<(ConversationId, Conversation)> {
    if !entry.file_type().ok()?.is_dir() {
        return None;
    }

    let file_name = entry.file_name();
    let Some(dir_name) = file_name.to_str() else {
        warn!(path = ?entry.path(), "Skipping directory with invalid name.");
        return None;
    };

    let conversation_id = match ConversationId::from_dirname(dir_name) {
        Ok(id) => id,
        Err(error) => {
            warn!(
                %error,
                path = ?entry.path(),
                "Failed to parse ConversationId from directory name. Skipping."
            );
            return None;
        }
    };

    let path = entry.path();

    let conversation = {
        let metadata_path = path.join(METADATA_FILE);
        match read_json::<Conversation>(&metadata_path) {
            Ok(c) => c,
            Err(error) => {
                warn!(
                    %error,
                    path = metadata_path.to_string_lossy().to_string(),
                    "Failed to load conversation metadata. Skipping."
                );
                return None;
            }
        }
    };

    Some((conversation_id, conversation))
}

fn remove_unused_conversation_dirs(
    id: &ConversationId,
    conversation_dir: &Path,
    workspace_conversations_dir: &Path,
    user_conversations_dir: &Path,
) -> Result<()> {
    // Gather all possible conversation directory names
    let mut dirs = vec![];
    for conversations_dir in &[workspace_conversations_dir, user_conversations_dir] {
        let pat = id.to_dirname(None);
        dirs.push(conversations_dir.join(&pat));
        for entry in fs::read_dir(conversations_dir).ok().into_iter().flatten() {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            if path
                .file_name()
                .is_none_or(|v| !v.to_string_lossy().starts_with(&format!("{pat}-")))
            {
                continue;
            }

            dirs.push(path);
        }
    }

    // Exclude the one we actually want to keep
    dirs.retain(|d| d != conversation_dir);

    // Remove the rest
    for dir in dirs {
        if !dir.exists() {
            continue;
        }

        fs::remove_dir_all(dir)?;
    }

    Ok(())
}

fn remove_deleted(root: &Path, dir: &Path, deleted: impl Iterator<Item = PathBuf>) -> Result<()> {
    for entry in deleted {
        let mut path = dir.join(entry);
        if path.is_file() {
            fs::remove_file(&path)?;
        } else if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            warn!(
                path = %path.display(),
                "File or directory marked for deletion not found. Skipping."
            );
        }

        // Remove empty parent directories, until we reach the root.
        while let Some(parent) = path.parent() {
            if parent.as_os_str() == "" || parent == root || !parent.is_dir() {
                break;
            }
            if fs::read_dir(parent)?.count() != 0 {
                break;
            }

            fs::remove_dir(parent)?;
            path = parent.to_path_buf();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        str::FromStr as _,
    };

    use jp_conversation::ConversationId;
    use tempfile::tempdir;
    use test_log::test;

    use super::*;

    #[test]
    fn test_storage_handles_missing_src() {
        let missing_path = PathBuf::from("./non_existent_jp_workspace_source_dir_abc123");
        assert!(!missing_path.exists());

        let storage = Storage::new(&missing_path).expect("must succeed");
        assert!(storage.root.is_dir());
        assert_eq!(fs::read_dir(&storage.root).unwrap().count(), 0);
        assert_eq!(storage.root, missing_path);

        fs::remove_dir_all(&missing_path).ok();
    }

    #[test]
    fn test_storage_new_errors_on_source_file() {
        let source_dir = tempdir().unwrap();
        let source_file_path = source_dir.path().join("source_is_a_file.txt");
        File::create(&source_file_path).unwrap();

        let result = Storage::new(&source_file_path);
        match result.expect_err("must fail") {
            Error::NotDir(path) => assert_eq!(path, source_file_path),
            _ => panic!("Expected Error::SourceNotDir"),
        }
    }

    #[test]
    fn test_load_user_conversations_metadata_reads_existing() {
        let original_dir = tempdir().unwrap();
        let user_dir = tempdir().unwrap();
        let name = "test";
        let id = "1234";
        let user_workspace_dir = user_dir.path().join(format!("{name}-{id}"));
        let meta_path = user_workspace_dir.join(METADATA_FILE);
        let existing_id = ConversationId::default();
        let existing_meta = ConversationsMetadata::new(existing_id);
        write_json(&meta_path, &existing_meta).unwrap();

        let storage = Storage::new(original_dir.path())
            .unwrap()
            .with_user_storage(user_dir.path(), name, id)
            .unwrap();
        let loaded_meta = storage.load_conversations_metadata().unwrap();
        assert_eq!(loaded_meta, existing_meta);
    }

    #[test]
    fn test_load_user_conversations_metadata_creates_default_when_missing() {
        let storage_dir = tempdir().unwrap();
        let user_dir = tempdir().unwrap();
        let name = "test";
        let id = "1234";

        let storage = Storage::new(storage_dir.path())
            .unwrap()
            .with_user_storage(user_dir.path(), name, id)
            .unwrap();
        let loaded_meta = storage.load_conversations_metadata().unwrap();
        let default_meta = ConversationsMetadata::default();

        assert_eq!(
            loaded_meta.active_conversation_id,
            default_meta.active_conversation_id
        );
    }

    #[test]
    fn test_conversation_dir_name_generation() {
        let id = ConversationId::from_str("jp-c17457886043-otvo8").unwrap();
        assert_eq!(id.to_dirname(None), "17457886043");
        assert_eq!(
            id.to_dirname(Some("Simple Title")),
            "17457886043-simple-title"
        );
        assert_eq!(
            id.to_dirname(Some(" Title with spaces & chars!")),
            "17457886043-title-with-spaces---chars" // Sanitized
        );
        assert_eq!(
            id.to_dirname(Some(
                "A very long title that definitely exceeds the sixty character limit for testing \
                 purposes"
            )),
            "17457886043-a-very-long-title-that-definitely-exceeds-the-sixty" // Truncated
        );
        assert_eq!(
            id.to_dirname(Some("")), // Empty title
            "17457886043"
        );
    }
}
