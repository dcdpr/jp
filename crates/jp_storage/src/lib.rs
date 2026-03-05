pub mod error;
pub mod value;

pub mod load;
pub mod persist;

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

    pub fn persist_conversations_metadata(
        &mut self,
        metadata: &ConversationsMetadata,
    ) -> Result<()> {
        // Only persist metadata if the active conversation exists.
        if self
            .load_conversation_stream(&metadata.active_conversation_id)
            .is_err()
        {
            return Ok(());
        }

        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        trace!(path = %metadata_path, "Persisting user conversations metadata.");

        write_json(&metadata_path, metadata)?;

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

// fn load_conversation_metadata(entry: &Utf8DirEntry) -> Option<(ConversationId, Conversation)> {
//     let conversation_id = load_conversation_id_from_entry(entry)?;
//
//     let path = entry.path();
//
//     let conversation = {
//         let metadata_path = path.join(METADATA_FILE);
//         match read_json::<Conversation>(&metadata_path) {
//             Ok(c) => c,
//             Err(error) => {
//                 warn!(
//                     %error,
//                     path = %metadata_path,
//                     "Failed to load conversation metadata. Skipping."
//                 );
//                 return None;
//             }
//         }
//     };
//
//     Some((conversation_id, conversation))
// }

fn load_conversation_id_from_entry(entry: &Utf8DirEntry) -> Option<ConversationId> {
    if !entry.file_type().ok()?.is_dir() {
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

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        str::FromStr as _,
    };

    use camino_tempfile::tempdir;
    use chrono::TimeZone as _;
    use jp_conversation::ConversationId;
    use serde_json::json;
    use test_log::test;

    use super::*;

    #[test]
    fn test_storage_handles_missing_src() {
        let missing_path = Utf8PathBuf::from("./non_existent_jp_workspace_source_dir_abc123");
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

    #[test]
    fn test_remove_ephemeral_conversations() {
        let storage_dir = tempdir().unwrap();
        let path = storage_dir.path();
        let convs = path.join(CONVERSATIONS_DIR);

        let id1 = ConversationId::try_from_deciseconds_str("17636257526").unwrap();
        let id2 = ConversationId::try_from_deciseconds_str("17636257527").unwrap();
        let id3 = ConversationId::try_from_deciseconds_str("17636257528").unwrap();
        let id4 = ConversationId::try_from_deciseconds_str("17636257529").unwrap();
        let id5 = ConversationId::try_from_deciseconds_str("17636257530").unwrap();

        let dir1 = convs.join(id1.to_dirname(None));
        fs::create_dir_all(&dir1).unwrap();
        write_json(
            &dir1.join("metadata.json"),
            &json!({
                "last_activated_at": Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
                "expires_at": Utc::now() - chrono::Duration::hours(1)
            }),
        )
        .unwrap();
        write_json(&dir1.join("events.json"), &json!([])).unwrap();

        let title = "hello world";
        let dir2 = convs.join(id2.to_dirname(Some(title)));
        fs::create_dir_all(&dir2).unwrap();
        write_json(
            &dir2.join("metadata.json"),
            &json!({
                "title": title,
                "last_activated_at": Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
                "expires_at": Utc::now() + chrono::Duration::hours(1)
            }),
        )
        .unwrap();
        write_json(&dir2.join("events.json"), &json!([])).unwrap();

        let dir3 = convs.join(id3.to_dirname(Some(title)));
        fs::create_dir_all(&dir3).unwrap();
        write_json(
            &dir3.join("metadata.json"),
            &json!({
                "title": title,
                "last_activated_at": Utc.with_ymd_and_hms(2023, 1, 1, 0, 0, 0).unwrap(),
                "expires_at": Utc::now() - chrono::Duration::hours(1)
            }),
        )
        .unwrap();
        write_json(&dir3.join("events.json"), &json!([])).unwrap();

        fs::create_dir_all(convs.join(id4.to_dirname(None))).unwrap();
        fs::create_dir_all(convs.join(id5.to_dirname(Some("foo")))).unwrap();

        let storage = Storage::new(path).unwrap();
        storage.remove_ephemeral_conversations(&[id4, id5]);

        assert!(!convs.join(id1.to_dirname(None)).exists());
        assert!(convs.join(id2.to_dirname(Some(title))).exists());
        assert!(!convs.join(id3.to_dirname(Some(title))).exists());
        assert!(convs.join(id4.to_dirname(None)).exists());
        assert!(convs.join(id5.to_dirname(Some("foo"))).exists());
    }
}
