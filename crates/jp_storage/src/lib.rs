pub mod backend;
pub mod error;
pub mod lock;
pub mod value;

pub mod load;
pub mod trash;
pub mod validate;

use std::{
    fs,
    io::{self, BufReader},
    time::SystemTime,
};

use camino::{Utf8DirEntry, Utf8Path, Utf8PathBuf};
use chrono::{DateTime, NaiveDateTime, Utc};
pub use error::Error;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
pub use load::LoadError;
use relative_path::RelativePath;
use tracing::{trace, warn};

use crate::{backend::Projection, error::Result, value::write_json};

pub(crate) const METADATA_FILE: &str = "metadata.json";
const EVENTS_FILE: &str = "events.json";
const BASE_CONFIG_FILE: &str = "base_config.json";
pub(crate) const CONVERSATIONS_DIR: &str = "conversations";
pub(crate) const ARCHIVE_DIR: &str = ".archive";

#[derive(Debug, Clone)]
struct Storage {
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
    const SESSIONS_DIR: &'static str = "sessions";

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

    /// Configure user-local storage for workspace `id` under `root`.
    ///
    /// The silo is located by ID suffix, so every worktree and clone of a
    /// workspace shares the single directory that already exists.
    /// When none does, a new `<slug>-<id>` directory is created: `slug`
    /// (typically the workspace directory name) is cosmetic, only ever names a
    /// *new* silo, is never validated, and an absent or empty slug yields a
    /// bare `<id>` directory.
    ///
    /// Before wiring up the directory, a one-time migration runs: any sibling
    /// silos for this workspace are merged in, and on first setup the
    /// workspace's conversations are imported so a durable user-local copy
    /// exists.
    pub fn with_user_storage(
        mut self,
        root: &Utf8Path,
        slug: Option<&str>,
        id: impl Into<String>,
    ) -> Result<Self> {
        let id: String = id.into();
        let (path, first_run) = resolve_user_dir(root, slug, &id);

        migrate_user_storage(root, &id, &path, &self.root, first_run)?;

        if path.exists() {
            if !path.is_dir() {
                return Err(Error::NotDir(path));
            }
        } else {
            fs::create_dir_all(&path)?;
            trace!(path = %path, "Created user storage directory.");
        }

        // Point the `storage` symlink back at the current workspace root,
        // repairing a link inherited from another worktree during migration.
        let link = path.join("storage");
        if link.is_symlink()
            && fs::read_link(&link).is_ok_and(|target| target.as_path() != self.root.as_std_path())
        {
            trace!(link = %link, "Re-pointing user storage symlink to current workspace.");
            remove_storage_symlink(&link)?;
        }
        if link.exists() {
            if !link.is_symlink() {
                return Err(Error::NotSymlink(link));
            }
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&self.root, &link)?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&self.root, &link)?;
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
    #[must_use]
    pub(crate) fn root_with_path(&self, path: &RelativePath) -> Utf8PathBuf {
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

    /// Persist a conversation to its configured storage roots.
    ///
    /// The durable user-local copy is always written.
    /// When `projection` is [`Projection::Projected`], a workspace copy is
    /// written too.
    /// With no user-local storage, the conversation is written to workspace
    /// storage only and `projection` is ignored.
    pub fn persist_conversation(
        &self,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
        projection: Projection,
    ) -> Result<()> {
        let conversations_path = RelativePath::new(CONVERSATIONS_DIR);
        let workspace_dir = self.root_with_path(conversations_path);

        match self.user_storage_with_path(conversations_path) {
            Some(user_dir) => {
                // Import an external (workspace-only) conversation into
                // user-local before its first durable write, so any
                // non-managed files in the committed copy survive.
                import_external_copy(id, metadata.title.as_deref(), &workspace_dir, &user_dir)?;
                // The durable user-local copy always holds the resolved base
                // config; the idempotent write skips it when unchanged.
                Self::persist_conversation_to(&user_dir, id, metadata, events)?;
                if projection == Projection::Projected {
                    Self::persist_conversation_to(&workspace_dir, id, metadata, events)?;
                } else {
                    // Local-only: drop any workspace projection (the
                    // `jp conversation edit --local` toggle).
                    remove_conversation_dirs(id, &workspace_dir)?;
                }
            }
            // No user-local storage: single-write to the workspace.
            None => Self::persist_conversation_to(&workspace_dir, id, metadata, events)?,
        }

        Ok(())
    }

    /// Persist a conversation into a single root's `conversations/` directory.
    ///
    /// The three managed files are written in place via the atomic, idempotent
    /// [`write_json`]: each file is replaced through a temp-file rename only
    /// when its bytes change, so unchanged files keep their modification time
    /// and the conversation directory's inode is stable (a shell `cd`'d into it
    /// is not invalidated).
    ///
    /// When the title changed, the existing directory is renamed into the new
    /// name first — a single atomic syscall that carries over its managed and
    /// non-managed files — and any other stale copies for the id are removed.
    ///
    /// The managed files are:
    ///
    /// - `metadata.json` — lightweight conversation metadata.
    /// - `base_config.json` — the `PartialAppConfig` snapshot, written from
    ///   the resolved in-memory config (the idempotent [`write_json`] skips the
    ///   rewrite when it is unchanged, so an untouched baseline keeps its
    ///   mtime).
    /// - `events.json` — the event stream (config deltas + conversation
    ///   events).
    fn persist_conversation_to(
        conversations_dir: &Utf8Path,
        id: &ConversationId,
        metadata: &Conversation,
        events: &ConversationStream,
    ) -> Result<()> {
        let dir_name = id.to_dirname(metadata.title.as_deref());
        let conv_dir = conversations_dir.join(&dir_name);

        // Bring any existing copy to the current directory name (e.g. after a
        // title change) and drop other stale copies for this id.
        reconcile_conversation_dir(id, conversations_dir, &conv_dir)?;
        fs::create_dir_all(&conv_dir)?;

        let (base_config, events_json) = events
            .to_parts()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

        write_json(&conv_dir.join(METADATA_FILE), metadata)?;

        // Write `base_config.json` before `events.json`. The loader picks the
        // current vs legacy format by the presence of `base_config.json`, so
        // for a legacy conversation's first upgrade this ordering ensures an
        // interrupted write never leaves the unreadable "current marker, no
        // base config" state. The worst interrupted state is a fresh
        // `base_config.json` beside a still-legacy `events.json`, which the
        // loader reads back without data loss.
        write_json(&conv_dir.join(BASE_CONFIG_FILE), &base_config)?;

        write_json(&conv_dir.join(EVENTS_FILE), &events_json)?;

        Ok(())
    }

    /// Move a conversation directory into the `.archive/` subdirectory of every
    /// root that holds it.
    ///
    /// A projected conversation lives in both roots, so each copy is archived;
    /// archiving only the first found would leave the other copy active.
    /// Creates each archive directory if needed.
    pub fn archive_conversation(&self, id: &ConversationId) -> Result<()> {
        let mut archived = false;
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let Some(conv_dir) = find_conversation_dir_path(root, id) else {
                continue;
            };

            let dirname = conv_dir
                .file_name()
                .expect("conversation dir has a name")
                .to_owned();

            let archive_dir = root.join(CONVERSATIONS_DIR).join(ARCHIVE_DIR);
            fs::create_dir_all(&archive_dir)?;

            let target = archive_dir.join(&dirname);
            if target.exists() {
                fs::remove_dir_all(&target)?;
            }
            fs::rename(&conv_dir, &target)?;
            archived = true;
        }

        if archived {
            Ok(())
        } else {
            Err(Error::ConversationNotFound(*id))
        }
    }

    /// Move a conversation directory out of `.archive/` back to the active
    /// conversations directory in every root that holds the archived copy.
    pub fn unarchive_conversation(&self, id: &ConversationId) -> Result<()> {
        let prefix = id.to_dirname(None);
        let mut unarchived = false;

        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let archive_dir = root.join(CONVERSATIONS_DIR).join(ARCHIVE_DIR);
            if !archive_dir.is_dir() {
                continue;
            }

            let entry = dir_entries(&archive_dir).find(|e| e.file_name().starts_with(&prefix));

            let Some(entry) = entry else {
                continue;
            };

            let dirname = entry.file_name().to_owned();
            let source = entry.into_path();
            let target = root.join(CONVERSATIONS_DIR).join(&dirname);

            if target.exists() {
                fs::remove_dir_all(&target)?;
            }
            fs::rename(&source, &target)?;
            unarchived = true;
        }

        if unarchived {
            Ok(())
        } else {
            Err(Error::ConversationNotFound(*id))
        }
    }

    /// Load metadata for a conversation in the archive partition.
    pub fn load_archived_conversation_metadata(
        &self,
        id: &ConversationId,
    ) -> std::result::Result<Conversation, crate::LoadError> {
        use crate::load::{LoadErrorInner, load_json};

        let prefix = id.to_dirname(None);
        for root in [Some(&self.root), self.user.as_ref()] {
            let Some(root) = root else {
                continue;
            };

            let archive_dir = root.join(CONVERSATIONS_DIR).join(ARCHIVE_DIR);
            let entry = dir_entries(&archive_dir).find(|e| e.file_name().starts_with(&prefix));

            let Some(entry) = entry else {
                continue;
            };

            let conv_dir = entry.into_path();
            let path = conv_dir.join(METADATA_FILE);
            if !path.is_file() {
                continue;
            }

            let mut conversation: Conversation = load_json(&path)?;
            (conversation.events_count, conversation.last_event_at) =
                crate::load::load_count_and_timestamp_events(&conv_dir).unwrap_or((0, None));

            return Ok(conversation);
        }

        Err(crate::LoadError::new(
            build_conversation_dir_prefix(&self.root, id),
            LoadErrorInner::MissingConversationMetadata(*id),
        ))
    }

    /// Remove a conversation's persisted data from disk.
    ///
    /// Removes all directories matching the conversation ID in both workspace
    /// and user storage.
    pub fn remove_conversation(&self, id: &ConversationId) -> Result<()> {
        let conversations_path = RelativePath::new(CONVERSATIONS_DIR);
        let conversations_dir = self.root_with_path(conversations_path);
        let user_conversations_dir = self.user_or_root_with_path(conversations_path);

        for dir in [&conversations_dir, &user_conversations_dir] {
            remove_conversation_dirs(id, dir)?;
        }

        Ok(())
    }

    /// Synchronize a projected conversation's user-local copy from its
    /// workspace copy.
    ///
    /// A managed editor command (`jp conversation edit --events` / `--metadata`
    /// / `--base-config`) edits the workspace copy of a projected or external
    /// conversation.
    /// Overwriting the user-local copy with it keeps both roots consistent
    /// immediately, rather than deferring to lazy mtime reconciliation on the
    /// next load.
    /// A local-only conversation (no workspace copy) is left untouched.
    pub fn sync_projection(&self, id: &ConversationId) -> Result<()> {
        let conversations_path = RelativePath::new(CONVERSATIONS_DIR);
        let Some(user_conversations) = self.user_storage_with_path(conversations_path) else {
            return Ok(());
        };
        let workspace_conversations = self.root_with_path(conversations_path);

        let prefix = id.to_dirname(None);
        let Some(workspace_conv) =
            find_normal_conversation_dir_path(&workspace_conversations, &prefix)
        else {
            return Ok(());
        };

        let dirname = workspace_conv
            .file_name()
            .expect("conversation dir has a name")
            .to_owned();

        remove_conversation_dirs(id, &user_conversations)?;
        fs::create_dir_all(&user_conversations)?;
        copy_dir_all(&workspace_conv, &user_conversations.join(&dirname))
    }

    /// Load a session mapping from user storage.
    ///
    /// Returns `Ok(None)` if user storage is not configured or the mapping file
    /// does not exist.
    /// Returns `Err` on I/O or parse errors.
    pub fn load_session_data<T: serde::de::DeserializeOwned>(
        &self,
        session_key: &str,
    ) -> Result<Option<T>> {
        let Some(sessions_dir) = self.user_storage_with_path(RelativePath::new(Self::SESSIONS_DIR))
        else {
            return Ok(None);
        };

        let path = sessions_dir.join(format!("{session_key}.json"));

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
        let sessions_dir = self
            .user_storage_with_path(RelativePath::new(Self::SESSIONS_DIR))
            .ok_or(Error::NotDir(Utf8PathBuf::from("<no user storage>")))?;

        let path = sessions_dir.join(format!("{session_key}.json"));

        write_json(&path, data)?;
        Ok(())
    }

    /// List orphaned lock files in user storage.
    ///
    /// A lock file is orphaned if no process holds the `flock` on it.
    /// This attempts a non-blocking lock; if it succeeds, the file is orphaned
    /// and its path is returned.
    #[must_use]
    pub fn list_orphaned_lock_files(&self) -> Vec<Utf8PathBuf> {
        let Some(locks_dir) = self.user_storage_with_path(RelativePath::new(lock::LOCKS_DIR))
        else {
            return vec![];
        };
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

    /// Build the expected conversation directory path.
    ///
    /// This constructs the path where a conversation *would* be stored,
    /// regardless of whether the directory exists.
    /// Used when creating new conversations or when the caller needs a stable
    /// path before persistence.
    #[must_use]
    pub fn build_conversation_dir(
        &self,
        id: &ConversationId,
        title: Option<&str>,
        user: bool,
    ) -> Utf8PathBuf {
        let conversations_path = RelativePath::new(CONVERSATIONS_DIR);
        let base = if user {
            self.user_or_root_with_path(conversations_path)
        } else {
            self.root_with_path(conversations_path)
        };

        base.join(id.to_dirname(title))
    }

    /// Find the directory path for a conversation by ID.
    ///
    /// Searches both workspace and user storage roots.
    /// Returns `None` if no directory matching the conversation ID exists.
    #[must_use]
    pub fn find_conversation_dir(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        [Some(&self.root), self.user.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|root| find_conversation_dir_path(root, id))
    }

    /// Find the conversation directory in the user-local store.
    ///
    /// Returns `None` when user-local storage is unconfigured, or when no
    /// directory there matches the ID.
    #[must_use]
    pub fn find_user_local_conversation_dir(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        let conversations = self.user_storage_with_path(RelativePath::new(CONVERSATIONS_DIR))?;
        find_normal_conversation_dir_path(&conversations, &id.to_dirname(None))
    }

    /// Path to a conversation's `events.json` file, if the directory exists.
    #[must_use]
    pub fn conversation_events_path(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.find_conversation_dir(id).map(|d| d.join(EVENTS_FILE))
    }

    /// Path to a conversation's `metadata.json` file, if the directory exists.
    #[must_use]
    pub fn conversation_metadata_path(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.find_conversation_dir(id)
            .map(|d| d.join(METADATA_FILE))
    }

    /// Path to a conversation's `base_config.json` file, if the directory
    /// exists.
    #[must_use]
    pub fn conversation_base_config_path(&self, id: &ConversationId) -> Option<Utf8PathBuf> {
        self.find_conversation_dir(id)
            .map(|d| d.join(BASE_CONFIG_FILE))
    }

    /// List session mapping files in user storage.
    #[must_use]
    pub fn list_session_files(&self) -> Vec<Utf8PathBuf> {
        let Some(sessions_dir) = self.user_storage_with_path(RelativePath::new(Self::SESSIONS_DIR))
        else {
            return vec![];
        };
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
}

/// Remove a `storage` symlink without following it.
///
/// On Windows a directory symlink is a reparse-point directory and must be
/// removed with `remove_dir`; `remove_file` returns "Access is denied".
/// On Unix `remove_file` unlinks the symlink itself.
fn remove_storage_symlink(link: &Utf8Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        fs::remove_dir(link)
    }
    #[cfg(not(windows))]
    {
        fs::remove_file(link)
    }
}

/// Resolve the user-local silo directory for workspace `id`.
///
/// Silos are located by ID suffix (`<id>` or `<slug>-<id>`), never by exact
/// name, so every worktree and clone of a workspace shares the one silo that
/// already exists regardless of the directory it was cloned into.
/// The returned flag is `true` when no silo exists yet and one must be created.
///
/// A new silo is named `<slug>-<id>` for human recognition; an absent or empty
/// `slug` yields a bare `<id>` directory.
/// The slug only ever names a *new* silo: an existing one is reused as-is and
/// never renamed.
///
/// When several silos already exist (legacy per-worktree directories), the one
/// whose name matches `<slug>-<id>` wins; otherwise the most recently modified
/// silo does.
fn resolve_user_dir(root: &Utf8Path, slug: Option<&str>, id: &str) -> (Utf8PathBuf, bool) {
    if let Some(dir) = choose_canonical_user_dir(&matching_user_dirs(root, id), slug, id) {
        return (dir, false);
    }

    let name = match slug.filter(|s| !s.is_empty()) {
        Some(slug) => format!("{slug}-{id}"),
        None => id.to_owned(),
    };
    (root.join(name), true)
}

/// List the user-local silo directories whose name resolves to workspace `id`.
fn matching_user_dirs(root: &Utf8Path, id: &str) -> Vec<Utf8PathBuf> {
    if !root.is_dir() {
        return vec![];
    }

    let suffix = format!("-{id}");
    dir_entries(root)
        .filter(|entry| {
            let name = entry.file_name();
            name == id || name.ends_with(suffix.as_str())
        })
        .map(Utf8DirEntry::into_path)
        .filter(|path| path.is_dir())
        .collect()
}

/// Pick the canonical silo among existing matches, or `None` when there are
/// none.
///
/// An exact `<slug>-<id>` match wins so a returning workspace keeps the
/// directory it recognizes; otherwise the most recently modified silo does,
/// breaking mtime ties by name for determinism.
fn choose_canonical_user_dir(
    dirs: &[Utf8PathBuf],
    slug: Option<&str>,
    id: &str,
) -> Option<Utf8PathBuf> {
    if let Some(slug) = slug.filter(|s| !s.is_empty()) {
        let wanted = format!("{slug}-{id}");
        if let Some(dir) = dirs.iter().find(|d| d.file_name() == Some(wanted.as_str())) {
            return Some(dir.clone());
        }
    }

    dirs.iter()
        .map(|dir| (dir_mtime(dir), dir))
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, dir)| dir.clone())
}

/// Migrate user-local storage into the chosen silo.
///
/// Merges any sibling silos for this workspace into `target` (kept as-is, never
/// renamed).
/// On the first run for a workspace it also imports the workspace's
/// conversations so they gain a durable user-local copy.
/// Later runs skip the import, leaving conversations committed by other
/// contributors to be imported lazily on first write rather than absorbed
/// eagerly here.
/// Workspace copies are never deleted, so an interrupted run loses no data.
fn migrate_user_storage(
    user_root: &Utf8Path,
    id: &str,
    target: &Utf8Path,
    workspace_root: &Utf8Path,
    first_run: bool,
) -> Result<()> {
    merge_sibling_user_dirs(user_root, id, target)?;

    if first_run {
        adopt_conversations(workspace_root, target, false)?;
    }

    Ok(())
}

/// Collapse every other silo for workspace `id` into `target`.
///
/// Sibling silos are matched by ID suffix, so legacy per-worktree directories
/// (`<name>-<id>`) and bare `<id>` directories alike are folded in,
/// conversation-by-conversation (the most recently modified copy wins on
/// conflict).
/// `target` itself is skipped and never renamed; once it has absorbed a
/// sibling's conversations and residual entries, the empty sibling is removed.
fn merge_sibling_user_dirs(user_root: &Utf8Path, id: &str, target: &Utf8Path) -> Result<()> {
    for dir in matching_user_dirs(user_root, id) {
        if dir == *target {
            continue;
        }

        trace!(sibling = %dir, target = %target, "Merging sibling user storage directory.");
        adopt_conversations(&dir, target, true)?;

        // Move any remaining entries (e.g. `sessions`) the target lacks. The
        // `storage` symlink is recreated by `with_user_storage`, conversations
        // are handled above, and anything still here is dropped with the dir.
        let residual: Vec<(String, Utf8PathBuf)> = dir_entries(&dir)
            .filter_map(|entry| {
                let name = entry.file_name().to_owned();
                (name != CONVERSATIONS_DIR && name != "storage").then(|| (name, entry.into_path()))
            })
            .collect();
        for (name, from) in residual {
            let dst = target.join(&name);
            if !dst.exists() {
                fs::rename(&from, &dst)?;
            }
        }

        fs::remove_dir_all(&dir)?;
    }

    Ok(())
}

/// Adopt every conversation under `src_root` into `dst_root`, covering both the
/// active and archive partitions.
///
/// With `move_src` the source directories are renamed into place; otherwise
/// they are copied so the originals survive (used when importing workspace
/// conversations, whose workspace copy must remain).
fn adopt_conversations(src_root: &Utf8Path, dst_root: &Utf8Path, move_src: bool) -> Result<()> {
    let src_active = src_root.join(CONVERSATIONS_DIR);
    let dst_active = dst_root.join(CONVERSATIONS_DIR);
    adopt_partition(&src_active, &dst_active, move_src)?;
    adopt_partition(
        &src_active.join(ARCHIVE_DIR),
        &dst_active.join(ARCHIVE_DIR),
        move_src,
    )?;
    Ok(())
}

/// Adopt every conversation directory in a single partition into `dst_part`,
/// keeping the most recently modified copy on conflict.
fn adopt_partition(src_part: &Utf8Path, dst_part: &Utf8Path, move_src: bool) -> Result<()> {
    if !src_part.is_dir() {
        return Ok(());
    }

    let entries: Vec<(ConversationId, Utf8PathBuf)> = dir_entries(src_part)
        .filter_map(|entry| {
            let id = load_conversation_id_from_entry(&entry)?;
            Some((id, entry.into_path()))
        })
        .collect();

    for (id, src_dir) in entries {
        adopt_conversation_dir(&src_dir, dst_part, &id, move_src)?;
    }

    Ok(())
}

/// Ensure `dst_part` holds conversation `id`, preferring the most recently
/// modified copy when both sides already have it.
fn adopt_conversation_dir(
    src_dir: &Utf8Path,
    dst_part: &Utf8Path,
    id: &ConversationId,
    move_src: bool,
) -> Result<()> {
    let existing = find_normal_conversation_dir_path(dst_part, &id.to_dirname(None));
    if let Some(dst_dir) = existing.as_ref()
        && dir_mtime(src_dir) <= dir_mtime(dst_dir)
    {
        return Ok(());
    }

    fs::create_dir_all(dst_part)?;
    if let Some(dst_dir) = existing {
        fs::remove_dir_all(&dst_dir)?;
    }

    let dst_dir = dst_part.join(src_dir.file_name().expect("conversation dir has a name"));
    if move_src {
        fs::rename(src_dir, &dst_dir)?;
    } else {
        copy_dir_all(src_dir, &dst_dir)?;
    }

    Ok(())
}

/// Remove every directory matching conversation `id` from a single
/// `conversations/` directory.
fn remove_conversation_dirs(id: &ConversationId, conversations_dir: &Utf8Path) -> Result<()> {
    let prefix = id.to_dirname(None);
    for entry in dir_entries(conversations_dir) {
        let name = entry.file_name().to_owned();
        let matches =
            (name == prefix || name.starts_with(&format!("{prefix}-"))) && entry.path().is_dir();
        if matches {
            fs::remove_dir_all(entry.into_path())?;
        }
    }
    Ok(())
}

/// Import a workspace-only conversation into user-local storage.
///
/// When the conversation exists in the workspace but not yet in user-local, its
/// whole directory is copied across — preserving non-managed files — to the
/// name the upcoming write will use.
/// A conversation already present in user-local, or one with no workspace copy,
/// is left untouched.
fn import_external_copy(
    id: &ConversationId,
    title: Option<&str>,
    workspace_conversations: &Utf8Path,
    user_conversations: &Utf8Path,
) -> Result<()> {
    let prefix = id.to_dirname(None);
    if find_normal_conversation_dir_path(user_conversations, &prefix).is_some() {
        return Ok(());
    }
    let Some(workspace_conv) = find_normal_conversation_dir_path(workspace_conversations, &prefix)
    else {
        return Ok(());
    };

    fs::create_dir_all(user_conversations)?;
    copy_dir_all(
        &workspace_conv,
        &user_conversations.join(id.to_dirname(title)),
    )
}

/// Recursively copy directory `src` into `dst`.
fn copy_dir_all(src: &Utf8Path, dst: &Utf8Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in dir_entries(src) {
        let to = dst.join(entry.file_name());
        let is_dir = entry.file_type().is_ok_and(|ty| ty.is_dir());
        let from = entry.into_path();
        if is_dir {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// The most recent modification time among a conversation directory's files.
///
/// Used to pick the freshest copy when the same conversation exists in two
/// roots.
/// Falls back to the directory's own mtime when it holds no readable files.
fn dir_mtime(dir: &Utf8Path) -> SystemTime {
    let mut newest: Option<SystemTime> = None;
    for entry in dir_entries(dir) {
        if let Ok(modified) = fs::metadata(entry.path()).and_then(|m| m.modified()) {
            newest = Some(newest.map_or(modified, |cur| cur.max(modified)));
        }
    }

    newest
        .or_else(|| fs::metadata(dir).and_then(|m| m.modified()).ok())
        .unwrap_or(SystemTime::UNIX_EPOCH)
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

/// Ensure conversation `id` lives at `target` within its `conversations/`
/// directory, then remove any other (stale) copies for the same id.
///
/// When the title changed, the existing copy lives under a different name;
/// renaming it into `target` is a single atomic syscall that carries over its
/// managed and non-managed files and preserves the directory inode.
/// Operating per root means a dual-write never touches the copy in the other
/// root.
fn reconcile_conversation_dir(
    id: &ConversationId,
    conversations_dir: &Utf8Path,
    target: &Utf8Path,
) -> Result<()> {
    let prefix = id.to_dirname(None);

    if !target.exists()
        && let Some(src) = conversation_dirs_for_id(conversations_dir, &prefix)
            .into_iter()
            .find(|dir| dir != target)
    {
        fs::rename(&src, target)?;
    }

    for dir in conversation_dirs_for_id(conversations_dir, &prefix) {
        if dir != *target {
            fs::remove_dir_all(&dir)?;
        }
    }

    Ok(())
}

/// Collect the normal (non-dot-prefixed) conversation directories matching
/// `prefix` (`{prefix}` or `{prefix}-{title}`) in a single `conversations/`
/// directory.
fn conversation_dirs_for_id(conversations_dir: &Utf8Path, prefix: &str) -> Vec<Utf8PathBuf> {
    let dash_prefix = format!("{prefix}-");
    dir_entries(conversations_dir)
        .filter(|entry| {
            let name = entry.file_name();
            (name == prefix || name.starts_with(&dash_prefix)) && entry.path().is_dir()
        })
        .map(Utf8DirEntry::into_path)
        .collect()
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
    find_normal_conversation_dir_path(&conversations, &prefix)
}

fn find_normal_conversation_dir_path(
    conversations: &Utf8Path,
    prefix: &str,
) -> Option<Utf8PathBuf> {
    dir_entries(conversations)
        .find(|entry| entry.file_name().starts_with(prefix))
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

pub(crate) fn load_conversation_id_from_entry(entry: &Utf8DirEntry) -> Option<ConversationId> {
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

// Internal methods for testing.
#[cfg(debug_assertions)]
impl Storage {
    /// Write a minimal valid conversation to the workspace storage root.
    ///
    /// Creates a conversation directory with valid `metadata.json`,
    /// `base_config.json`, and `events.json` files.
    /// For test fixture setup only.
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
    /// Searches both workspace and user storage roots.
    /// Returns `None` if the conversation or its events file doesn't exist.
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
    /// metadata.json" check during validation.
    /// For test fixture setup only.
    #[doc(hidden)]
    pub fn create_test_conversation_dir(&self, dirname: &str) {
        let dir = self.root.join(CONVERSATIONS_DIR).join(dirname);
        fs::create_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
