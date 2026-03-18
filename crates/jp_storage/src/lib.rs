pub mod error;
pub mod value;

pub mod load;
pub mod persist;
pub mod trash;
pub mod validate;

use std::{cell::OnceCell, fs, io::BufReader, iter};

use camino::{FromPathBufError, Utf8DirEntry, Utf8Path, Utf8PathBuf};
use chrono::{DateTime, NaiveDateTime, Utc};
pub use error::Error;
use jp_conversation::{Conversation, ConversationId, ConversationStream, ConversationsMetadata};
use jp_id::Id as _;
use jp_tombmap::TombMap;
pub use load::LoadError;
use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};
use relative_path::RelativePath;
use tracing::{trace, warn};

use crate::{error::Result, value::write_json};

pub const METADATA_FILE: &str = "metadata.json";
const EVENTS_FILE: &str = "events.json";
pub const CONVERSATIONS_DIR: &str = "conversations";

#[derive(Debug)]
pub struct Storage {
    /// The path to the original storage directory.
    root: Utf8PathBuf,

    /// The path to the user storage directory.
    ///
    /// This is used (among other things) to store the active conversation id
    /// that are tied to the current user.
    ///
    /// If unset, user storage is disabled.
    user: Option<Utf8PathBuf>,
}

impl Storage {
    /// Creates a new Storage instance by creating a temporary directory and
    /// copying the contents of `root` into it.
    pub fn new(root: impl Into<Utf8PathBuf>) -> Result<Self> {
        // Create root storage directory, if needed.
        let root: Utf8PathBuf = root.into();
        if root.exists() {
            if !root.is_dir() {
                return Err(Error::NotDir(root));
            }
        } else {
            fs::create_dir_all(&root)?;
            trace!(path = %root, "Created storage directory.");
        }

        Ok(Self { root, user: None })
    }

    pub fn with_user_storage(
        mut self,
        root: &Utf8Path,
        name: impl AsRef<str>,
        id: impl Into<String>,
    ) -> Result<Self> {
        let name = name.as_ref();
        let id: String = id.into();
        let dirname = format!("{name}-{id}");
        let mut path = root.join(&dirname);

        // Create user storage directory, if needed.
        if root.exists()
            && let Some(existing_path) = fs::read_dir(root)?.find_map(|entry| {
                let path = entry.ok()?.path();
                path.to_string_lossy().ends_with(&id).then_some(path)
            })
        {
            let mut existing_path: Utf8PathBuf = existing_path
                .try_into()
                .map_err(FromPathBufError::into_io_error)?;

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
                    old = %existing_path,
                    new = %new_path,
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
            trace!(path = %path, "Created user storage directory.");
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
    pub fn path(&self) -> &Utf8Path {
        &self.root
    }

    /// Returns the path to the user storage directory, if configured.
    #[must_use]
    pub fn user_storage_path(&self) -> Option<&Utf8Path> {
        self.user.as_deref()
    }

    /// Return the absolute path to the given relative path, starting from the
    /// storage root.
    #[expect(clippy::missing_panics_doc)]
    #[must_use]
    pub fn root_with_path(&self, path: &RelativePath) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(path.to_logical_path(&self.root))
            .expect("relative path is Unicode, as is the root")
    }

    /// Return the absolute path to the given relative path, starting from the
    /// user storage directory.
    #[must_use]
    pub fn user_storage_with_path(&self, path: &RelativePath) -> Option<Utf8PathBuf> {
        let user = self.user.as_deref()?;
        path.to_logical_path(user).try_into().ok()
    }

    /// Return the absolute path to the given relative path, starting from the
    /// user or workspace storage directory.
    #[must_use]
    pub fn user_or_root_with_path(&self, path: &RelativePath) -> Utf8PathBuf {
        self.user_storage_with_path(path)
            .unwrap_or_else(|| self.root_with_path(path))
    }

    pub fn persist_conversations_and_events(
        &mut self,
        conversations: &TombMap<ConversationId, OnceCell<Conversation>>,
        events: &TombMap<ConversationId, OnceCell<ConversationStream>>,
        active_conversation_id: &ConversationId,
        active_conversation: &Conversation,
    ) -> Result<()> {
        let root = self.root.as_path();
        let user = self.user.as_deref().unwrap_or(root);

        let conversations_dir = root.join(CONVERSATIONS_DIR);
        let user_conversations_dir = user.join(CONVERSATIONS_DIR);

        trace!(
            global = %conversations_dir,
            user = %user_conversations_dir,
            "Persisting conversations."
        );

        // Append the active conversation to the list of conversations to
        // persist.
        let all_conversations = conversations
            .iter()
            .filter_map(|(id, conversation)| conversation.get().map(|v| (id, v)))
            .chain(iter::once((active_conversation_id, active_conversation)));

        for (id, conversation) in all_conversations {
            let dir_name = id.to_dirname(conversation.title.as_deref());
            let conv_dir = if conversation.user {
                user_conversations_dir.join(dir_name)
            } else {
                conversations_dir.join(dir_name)
            };

            // If the conversation is being modified (e.g. moved or renamed) and
            // its events are not yet loaded in memory, we load them from disk
            // before we potentially delete the old directory.
            let mut stream = events.get(id).and_then(|v| v.get());
            let loaded_stream;
            if stream.is_none()
                && (conversations.is_modified(id) || id == active_conversation_id)
                && let Ok(s) = self.load_conversation_stream(id)
            {
                loaded_stream = Some(s);
                stream = loaded_stream.as_ref();
            }

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

            // Don't write metadata for non-existent conversations.
            let Some(stream) = stream else {
                continue;
            };

            fs::create_dir_all(&conv_dir)?;

            // Write conversation metadata
            let meta_path = conv_dir.join(METADATA_FILE);
            write_json(&meta_path, conversation)?;

            let events_path = conv_dir.join(EVENTS_FILE);
            write_json(&events_path, stream)?;
        }

        // Don't mark active conversation as removed.
        let removed_ids = conversations
            .removed_keys()
            .filter(|&id| id != active_conversation_id)
            .collect::<Vec<_>>();

        for dir in [&conversations_dir, &user_conversations_dir] {
            let mut deleted = Vec::new();
            for entry in dir_entries(&dir) {
                let path = entry.path();
                let dir_matches_id = path.file_name().is_some_and(|v| {
                    removed_ids.iter().any(|d| {
                        let removed_id = d.target_id();

                        v == &*removed_id || v.starts_with(&format!("{removed_id}-"))
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

    pub fn persist_conversations_metadata(&self, metadata: &ConversationsMetadata) -> Result<()> {
        // Only persist metadata if the active conversation has a directory on
        // disk. This prevents writing a metadata file that references a
        // conversation that was never persisted (e.g., a fresh in-memory
        // conversation on first run).
        let id = &metadata.active_conversation_id;
        let exists = [Some(&self.root), self.user.as_ref()]
            .into_iter()
            .flatten()
            .any(|root| find_conversation_dir_path(root, id).is_some());

        if !exists {
            return Ok(());
        }

        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        trace!(path = %metadata_path, "Persisting user conversations metadata.");

        write_json(&metadata_path, metadata)?;

        Ok(())
    }

    /// Remove the global conversations metadata file.
    ///
    /// After removal, [`load_conversations_metadata`] will return default
    /// metadata.
    ///
    /// [`load_conversations_metadata`]: Self::load_conversations_metadata
    pub fn remove_conversations_metadata(&self) -> Result<()> {
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);

        if metadata_path.is_file() {
            fs::remove_file(&metadata_path)?;
        }

        Ok(())
    }

    /// Remove all ephemeral conversations, except the active one.
    pub fn remove_ephemeral_conversations(&self, skip: &[ConversationId]) {
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let path = root.join(CONVERSATIONS_DIR);
            dir_entries(&path)
                .collect::<Vec<_>>()
                .into_par_iter()
                .filter_map(|entry| {
                    let id = load_conversation_id_from_entry(&entry)?;
                    if skip.contains(&id) {
                        return None;
                    }

                    let path = entry.into_path();
                    let expiring_ts = get_expiring_timestamp(&path)?;
                    if expiring_ts > Utc::now() {
                        return None;
                    }

                    Some(path)
                })
                .for_each(|path| {
                    if let Err(error) = fs::remove_dir_all(&path) {
                        warn!(
                            path = %path,
                            error = error.to_string(),
                            "Failed to remove ephemeral conversation."
                        );
                    }
                });
        }
    }
}

fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
}

/// Get the `expires_at` timestamp from the conversation metadata file, if the
/// file exists, and the `expires_at` timestamp is set.
///
/// This is a specialized function that ONLY parses the `expires_at` field in
/// the JSON metadata file, for performance reasons.
fn get_expiring_timestamp(root: &Utf8Path) -> Option<DateTime<Utc>> {
    #[derive(serde::Deserialize)]
    struct RawConversation {
        expires_at: Option<Box<serde_json::value::RawValue>>,
    }
    let path = root.join(METADATA_FILE);
    let file = fs::File::open(&path).ok()?;
    let reader = BufReader::new(file);

    let conversation: RawConversation = match serde_json::from_reader(reader) {
        Ok(conversation) => conversation,
        Err(error) => {
            warn!(%error, path = %path, "Error parsing JSON metadata file.");
            return None;
        }
    };

    let ts = conversation.expires_at?;
    let ts = ts.get();
    if ts.len() < 2 || !ts.starts_with('"') || !ts.ends_with('"') {
        return None;
    }

    parse_datetime(&ts[1..ts.len() - 1])
}

fn dir_entries(path: impl AsRef<Utf8Path>) -> impl Iterator<Item = Utf8DirEntry> {
    path.as_ref()
        .read_dir_utf8()
        .into_iter()
        .flatten()
        .filter_map(std::result::Result::ok)
}

fn find_conversation_dir_path(root: &Utf8Path, id: &ConversationId) -> Option<Utf8PathBuf> {
    dir_entries(root.join(CONVERSATIONS_DIR))
        .find(|entry| entry.file_name().starts_with(&id.to_dirname(None)))
        .map(Utf8DirEntry::into_path)
}

/// Builds the path prefix to the directory for a given conversation ID.
///
/// This path does NOT include the optional conversation title, so it can't be
/// used directly to load any conversation data, but can be used as a starting
/// point for building the full path to a conversation directory.
///
/// For example, if a conversation `x` has the title `foo`, then
/// `build_conversation_dir_prefix` will return `{root}/conversations/x`, but
/// the actual conversation directory will be `{root}/conversations/x-foo`.
fn build_conversation_dir_prefix(root: &Utf8Path, id: &ConversationId) -> Utf8PathBuf {
    root.join(CONVERSATIONS_DIR).join(id.to_dirname(None))
}

fn load_conversation_id_from_entry(entry: &Utf8DirEntry) -> Option<ConversationId> {
    if !entry.file_type().ok()?.is_dir() {
        return None;
    }

    // Skip dot-prefixed entries (e.g., .trash/).
    if entry.file_name().starts_with('.') {
        return None;
    }

    ConversationId::try_from_dirname(entry.file_name())
        .inspect_err(|error| {
            warn!(
                %error,
                path = ?entry.path(),
                "Failed to parse ConversationId from directory name. Skipping."
            );
        })
        .ok()
}

fn remove_unused_conversation_dirs(
    id: &ConversationId,
    conversation_dir: &Utf8Path,
    workspace_conversations_dir: &Utf8Path,
    user_conversations_dir: &Utf8Path,
) -> Result<()> {
    // Gather all possible conversation directory names
    let mut dirs = vec![];
    for conversations_dir in &[workspace_conversations_dir, user_conversations_dir] {
        let pat = id.to_dirname(None);
        dirs.push(conversations_dir.join(&pat));
        for entry in dir_entries(conversations_dir) {
            let path = entry.into_path();
            if !path.is_dir() {
                continue;
            }
            if path
                .file_name()
                .is_none_or(|v| !v.starts_with(&format!("{pat}-")))
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

fn remove_deleted(
    root: &Utf8Path,
    dir: &Utf8Path,
    deleted: impl Iterator<Item = Utf8PathBuf>,
) -> Result<()> {
    for entry in deleted {
        let mut path = dir.join(entry);
        if path.is_file() {
            fs::remove_file(&path)?;
        } else if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else {
            warn!(
                path = %path,
                "File or directory marked for deletion not found. Skipping."
            );
        }

        // Remove empty parent directories, until we reach the root.
        while let Some(parent) = path.parent() {
            if parent.as_os_str() == "" || parent == root || !parent.is_dir() {
                break;
            }
            if dir_entries(parent).count() != 0 {
                break;
            }

            fs::remove_dir(parent)?;
            path = parent.to_path_buf();
        }
    }

    Ok(())
}

// Internal methods for testing.
#[cfg(debug_assertions)]
impl Storage {
    /// Write a minimal valid conversation to the workspace storage root.
    ///
    /// Creates a conversation directory with valid `metadata.json` and
    /// `events.json` files. For test fixture setup only.
    #[doc(hidden)]
    pub fn write_test_conversation(&self, id: &ConversationId, conversation: &Conversation) {
        let dir = self
            .root
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(conversation.title.as_deref()));
        fs::create_dir_all(&dir).unwrap();
        write_json(&dir.join(METADATA_FILE), conversation).unwrap();
        write_json(&dir.join(EVENTS_FILE), &ConversationStream::new_test()).unwrap();
    }

    /// Write the global conversations metadata file.
    ///
    /// Writes to user storage if configured, otherwise the workspace root.
    /// For test fixture setup only.
    #[doc(hidden)]
    pub fn write_test_conversations_metadata(&self, active_id: ConversationId) {
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_json(&path, &ConversationsMetadata::new(active_id)).unwrap();
    }

    /// Read the raw persisted events file content for a conversation.
    ///
    /// Searches both workspace and user storage roots. Returns `None` if
    /// the conversation or its events file doesn't exist.
    /// For test assertions only.
    #[doc(hidden)]
    #[must_use]
    pub fn read_test_events_raw(&self, id: &ConversationId) -> Option<String> {
        [Some(&self.root), self.user.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|root| {
                let dir = find_conversation_dir_path(root, id)?;
                fs::read_to_string(dir.join(EVENTS_FILE)).ok()
            })
    }

    /// Create an empty conversation directory that will fail validation.
    ///
    /// The directory contains no files, so it will fail the "missing
    /// metadata.json" check during validation. For test fixture setup only.
    #[doc(hidden)]
    pub fn create_test_conversation_dir(&self, dirname: &str) {
        let dir = self.root.join(CONVERSATIONS_DIR).join(dirname);
        fs::create_dir_all(&dir).unwrap();
    }

    /// Write corrupt (unparseable) data to the global conversations metadata
    /// file. For test fixture setup only.
    #[doc(hidden)]
    pub fn write_test_corrupt_conversations_metadata(&self) {
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "corrupt").unwrap();
    }

    /// Returns whether the global conversations metadata file exists on disk.
    /// For test assertions only.
    #[doc(hidden)]
    #[must_use]
    pub fn conversations_metadata_exists(&self) -> bool {
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        root.join(CONVERSATIONS_DIR).join(METADATA_FILE).is_file()
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
