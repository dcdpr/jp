pub mod error;
pub mod lock;
pub mod value;

pub mod load;
pub mod trash;
pub mod validate;

use std::{
    fs,
    io::{self, BufReader},
    thread,
    time::Duration,
};

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
const BASE_CONFIG_FILE: &str = "base_config.json";
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
    /// Uses a staging directory and atomic directory swap for write safety:
    ///
    /// 1. Write all files into a `.staging-{name}` directory.
    /// 2. Copy non-managed files from the existing conversation directory
    ///    (e.g. `QUERY_MESSAGE.md`) into the staging directory.
    /// 3. Rename the existing directory to `.old-{name}`.
    /// 4. Rename the staging directory to the final name.
    /// 5. Remove the `.old-{name}` backup.
    ///
    /// If any write in step 1 fails, the staging directory is removed and
    /// the existing conversation is untouched. The rename in step 4 is a
    /// single syscall, so readers never see a partially-written directory.
    ///
    /// Recovery: if the process crashes between steps 3 and 4, the next
    /// startup's validation pass detects the `.old-*` / `.staging-*` pair
    /// and completes or rolls back the swap.
    ///
    /// The conversation is stored as three managed files:
    /// - `metadata.json` — lightweight conversation metadata.
    /// - `base_config.json` — the initial `PartialAppConfig` snapshot, written
    ///   once at creation time. Subsequent persists preserve the existing file.
    /// - `events.json` — the event stream (config deltas + conversation
    ///   events).
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
        let parent_dir = if metadata.user {
            &user_conversations_dir
        } else {
            &conversations_dir
        };
        let conv_dir = parent_dir.join(&dir_name);

        // Remove stale directories from previous titles or storage locations.
        remove_stale_conversation_dirs(id, &conv_dir, &conversations_dir, &user_conversations_dir)?;

        let (base_config, events_json) = events
            .to_parts()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

        // Step 1: Write all managed files into a staging directory.
        let staging_dir = parent_dir.join(staging_dir_name(&dir_name));
        if staging_dir.exists() {
            fs::remove_dir_all(&staging_dir)?;
        }
        fs::create_dir_all(&staging_dir)?;

        let result = (|| -> Result<()> {
            write_json(&staging_dir.join(METADATA_FILE), metadata)?;

            // base_config.json is immutable after creation. If the conversation
            // already has one on disk, copy it into the staging dir so user
            // edits are preserved. Otherwise write a fresh one.
            let existing_base_config = conv_dir.join(BASE_CONFIG_FILE);
            if existing_base_config.is_file() {
                fs::copy(&existing_base_config, staging_dir.join(BASE_CONFIG_FILE))?;
            } else {
                write_json(&staging_dir.join(BASE_CONFIG_FILE), &base_config)?;
            }

            write_json(&staging_dir.join(EVENTS_FILE), &events_json)?;

            // Step 2: Copy non-managed files from the existing conversation
            // directory into the staging directory.
            if conv_dir.is_dir() {
                copy_non_managed_files(&conv_dir, &staging_dir)?;
            }

            Ok(())
        })();

        if let Err(e) = result {
            drop(fs::remove_dir_all(&staging_dir));
            return Err(e);
        }

        // Step 3: Move the old directory out of the way.
        let old_dir = parent_dir.join(old_dir_name(&dir_name));
        if old_dir.exists() {
            fs::remove_dir_all(&old_dir)?;
        }
        if conv_dir.is_dir() {
            fs::rename(&conv_dir, &old_dir)?;
        }

        // Step 4: Rename staging → final (single atomic syscall).
        fs::rename(&staging_dir, &conv_dir)?;

        // Step 5: Remove the backup.
        if old_dir.is_dir() {
            drop(fs::remove_dir_all(&old_dir));
        }

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

/// The dot-prefixed staging directory name used during atomic conversation
/// persistence.
pub(crate) const STAGING_PREFIX: &str = ".staging-";

/// The dot-prefixed backup directory name used during the atomic swap in
/// [`Storage::persist_conversation`]. Holds the old conversation directory
/// between the two renames.
pub(crate) const OLD_PREFIX: &str = ".old-";

/// Build the staging directory name for a conversation dir name.
fn staging_dir_name(dir_name: &str) -> String {
    format!("{STAGING_PREFIX}{dir_name}")
}

/// Build the old/backup directory name for a conversation dir name.
fn old_dir_name(dir_name: &str) -> String {
    format!("{OLD_PREFIX}{dir_name}")
}

/// The set of files managed by [`Storage::persist_conversation`].
const MANAGED_FILES: &[&str] = &[METADATA_FILE, BASE_CONFIG_FILE, EVENTS_FILE];

/// Copy non-managed files from `src` to `dst`.
///
/// Files whose names match [`MANAGED_FILES`] are skipped — those are written
/// fresh by the persistence logic. Everything else (e.g. `QUERY_MESSAGE.md`)
/// is copied so it survives the directory swap.
fn copy_non_managed_files(src: &Utf8Path, dst: &Utf8Path) -> Result<()> {
    for entry in dir_entries(src) {
        let name = entry.file_name().to_owned();
        if MANAGED_FILES.contains(&name.as_str()) {
            continue;
        }
        let src_path = entry.into_path();
        if src_path.is_file() {
            fs::copy(&src_path, dst.join(&name))?;
        }
    }
    Ok(())
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
    let prefix = id.to_dirname(None);
    let conversations = root.join(CONVERSATIONS_DIR);

    // Fast path: find the normal (non-dot-prefixed) directory.
    if let Some(path) = find_normal_conversation_dir_path(&conversations, &prefix) {
        return Some(path);
    }

    // If the directory isn't found but an in-flight persist directory exists
    // (`.old-*` or `.staging-*`), another process is mid-atomic-swap. The
    // rename gap is nanoseconds, so a brief retry is sufficient.
    if !has_inflight_persist_dir(&conversations, &prefix) {
        return None;
    }

    for _ in 0..10 {
        thread::sleep(Duration::from_millis(1));

        if let Some(path) = find_normal_conversation_dir_path(&conversations, &prefix) {
            return Some(path);
        }
    }

    None
}

fn find_normal_conversation_dir_path(
    conversations: &Utf8Path,
    prefix: &str,
) -> Option<Utf8PathBuf> {
    dir_entries(conversations)
        .find(|entry| entry.file_name().starts_with(prefix))
        .map(Utf8DirEntry::into_path)
}

fn has_inflight_persist_dir(conversations: &Utf8Path, prefix: &str) -> bool {
    dir_entries(conversations).any(|entry| {
        let name = entry.file_name();
        name.strip_prefix(OLD_PREFIX)
            .or_else(|| name.strip_prefix(STAGING_PREFIX))
            .is_some_and(|stripped| stripped.starts_with(prefix))
    })
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

    // Skip dot-prefixed entries (e.g., .trash/, .old-*, .staging-*).
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

/// Extract a conversation ID from an in-flight persist directory name.
///
/// Recognizes `.old-*` and `.staging-*` directories created by the atomic
/// swap in [`Storage::persist_conversation`]. Returns `None` for normal
/// entries, `.trash/`, or anything else.
fn load_inflight_conversation_id(entry: &Utf8DirEntry) -> Option<ConversationId> {
    if !entry.file_type().ok()?.is_dir() {
        return None;
    }

    let name = entry
        .file_name()
        .strip_prefix(OLD_PREFIX)
        .or_else(|| entry.file_name().strip_prefix(STAGING_PREFIX))?;

    ConversationId::try_from_dirname(name).ok()
}

// Internal methods for testing.
#[cfg(debug_assertions)]
impl Storage {
    /// Write a minimal valid conversation to the workspace storage root.
    ///
    /// Creates a conversation directory with valid `metadata.json`,
    /// `base_config.json`, and `events.json` files. For test fixture setup
    /// only.
    #[doc(hidden)]
    pub fn write_test_conversation(&self, id: &ConversationId, conversation: &Conversation) {
        let dir = self
            .root
            .join(CONVERSATIONS_DIR)
            .join(id.to_dirname(conversation.title.as_deref()));
        fs::create_dir_all(&dir).unwrap();
        write_json(&dir.join(METADATA_FILE), conversation).unwrap();

        let stream = ConversationStream::new_test();
        let (base_config, events) = stream.to_parts().unwrap();
        write_json(&dir.join(BASE_CONFIG_FILE), &base_config).unwrap();
        write_json(&dir.join(EVENTS_FILE), &events).unwrap();
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
