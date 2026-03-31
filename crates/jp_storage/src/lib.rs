pub mod error;
pub mod lock;
pub mod value;

pub mod load;
pub mod persist;
pub mod trash;
pub mod validate;

use std::{fs, io::BufReader};

use camino::{FromPathBufError, Utf8DirEntry, Utf8Path, Utf8PathBuf};
use chrono::{DateTime, NaiveDateTime, Utc};
pub use error::Error;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
pub use load::LoadError;
use rayon::iter::{IntoParallelIterator as _, ParallelIterator as _};
use relative_path::RelativePath;
use tracing::{trace, warn};

use crate::{error::Result, value::write_json};

pub const METADATA_FILE: &str = "metadata.json";
const EVENTS_FILE: &str = "events.json";
pub const CONVERSATIONS_DIR: &str = "conversations";

#[derive(Debug, Clone)]
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

    /// Persist a single conversation's metadata and events to disk.
    ///
    /// Handles directory naming, stale directory cleanup (when a conversation
    /// is renamed or moved between workspace/user storage), and atomic writes.
    pub fn persist_conversation(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()> {
        let conversations_dir = self.root.join(CONVERSATIONS_DIR);
        let user = self.user.as_deref().unwrap_or(&self.root);
        let user_conversations_dir = user.join(CONVERSATIONS_DIR);

        let dir_name = id.to_dirname(metadata.title.as_deref());
        let conv_dir = if metadata.user {
            user_conversations_dir.join(&dir_name)
        } else {
            conversations_dir.join(&dir_name)
        };

        // Remove stale directories from previous titles or storage locations.
        remove_stale_conversation_dirs(id, &conv_dir, &conversations_dir, &user_conversations_dir)?;

        fs::create_dir_all(&conv_dir)?;
        write_json(&conv_dir.join(METADATA_FILE), metadata)?;
        write_json(&conv_dir.join(EVENTS_FILE), events)?;

        Ok(())
    }

    /// Remove a conversation's persisted data from disk.
    ///
    /// Removes all directories matching the conversation ID in both workspace
    /// and user storage.
    pub fn remove_conversation(&self, id: &ConversationId) -> Result<()> {
        let conversations_dir = self.root.join(CONVERSATIONS_DIR);
        let user = self.user.as_deref().unwrap_or(&self.root);
        let user_conversations_dir = user.join(CONVERSATIONS_DIR);
        let prefix = id.to_dirname(None);

        for dir in [&conversations_dir, &user_conversations_dir] {
            if !dir.exists() {
                continue;
            }
            for entry in dir.read_dir_utf8().into_iter().flatten().flatten() {
                let name = entry.file_name();
                if (name == prefix || name.starts_with(&format!("{prefix}-")))
                    && entry.path().is_dir()
                {
                    fs::remove_dir_all(entry.path())?;
                }
            }
        }

        Ok(())
    }

    const SESSIONS_DIR: &'static str = "sessions";

    /// Load a session mapping from user storage.
    ///
    /// Returns `Ok(None)` if user storage is not configured or the mapping
    /// file does not exist. Returns `Err` on I/O or parse errors.
    pub fn load_session_data<T: serde::de::DeserializeOwned>(
        &self,
        session_key: &str,
    ) -> Result<Option<T>> {
        let Some(user) = self.user.as_deref() else {
            return Ok(None);
        };

        let path = user
            .join(Self::SESSIONS_DIR)
            .join(format!("{session_key}.json"));

        if !path.is_file() {
            return Ok(None);
        }

        value::read_json(&path).map(Some)
    }

    /// Save a session mapping to user storage.
    ///
    /// Creates the sessions directory if it does not exist.
    /// Returns `Err` if user storage is not configured.
    pub fn save_session_data<T: serde::Serialize>(
        &self,
        session_key: &str,
        data: &T,
    ) -> Result<()> {
        let user = self
            .user
            .as_deref()
            .ok_or(Error::NotDir(Utf8PathBuf::from("<no user storage>")))?;

        let path = user
            .join(Self::SESSIONS_DIR)
            .join(format!("{session_key}.json"));

        write_json(&path, data)?;
        Ok(())
    }

    /// List orphaned lock files in user storage.
    ///
    /// A lock file is orphaned if no process holds the `flock` on it.
    /// This attempts a non-blocking lock; if it succeeds, the file is
    /// orphaned and its path is returned.
    #[must_use]
    pub fn list_orphaned_lock_files(&self) -> Vec<Utf8PathBuf> {
        let Some(user) = self.user.as_deref() else {
            return vec![];
        };

        let locks_dir = user.join(lock::LOCKS_DIR);
        dir_entries(&locks_dir)
            .filter_map(|entry| {
                let path = entry.into_path();
                if path.extension().is_none_or(|ext| ext != "lock") {
                    return None;
                }

                // Try to acquire the lock. If we succeed, nobody holds it
                // and the file is orphaned.
                lock::is_orphaned_lock(&path).then_some(path)
            })
            .collect()
    }

    /// List session mapping files in user storage.
    #[must_use]
    pub fn list_session_files(&self) -> Vec<Utf8PathBuf> {
        let Some(user) = self.user.as_deref() else {
            return vec![];
        };

        let sessions_dir = user.join(Self::SESSIONS_DIR);
        dir_entries(&sessions_dir)
            .filter_map(|entry| {
                let path = entry.into_path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    Some(path)
                } else {
                    None
                }
            })
            .collect()
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

/// Remove stale conversation directories left over from renames or
/// workspace/user storage moves.
fn remove_stale_conversation_dirs(
    id: &ConversationId,
    target_dir: &Utf8Path,
    workspace_dir: &Utf8Path,
    user_dir: &Utf8Path,
) -> Result<()> {
    let prefix = id.to_dirname(None);
    let mut stale = vec![];

    for parent in [workspace_dir, user_dir] {
        stale.push(parent.join(&prefix));
        if let Ok(entries) = parent.read_dir_utf8() {
            for entry in entries.flatten() {
                let path = entry.into_path();
                if path.is_dir()
                    && path
                        .file_name()
                        .is_some_and(|n| n.starts_with(&format!("{prefix}-")))
                {
                    stale.push(path);
                }
            }
        }
    }

    stale.retain(|d| d != target_dir);

    for dir in stale {
        if dir.exists() {
            fs::remove_dir_all(dir)?;
        }
    }

    Ok(())
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
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
