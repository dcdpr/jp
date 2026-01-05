//! JP Workspace: A crate for managing LLM-assisted code conversations
//!
//! This crate provides data models and storage operations for the JP workspace,
//! a CLI tool for managing LLM-assisted code conversations with fine-grained
//! control over context and behavior.

mod error;
mod id;
mod state;

use std::{
    cell::OnceCell,
    iter,
    path::{Path, PathBuf},
    sync::Arc,
};

pub use error::Error;
use error::Result;
pub use id::Id;
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_storage::Storage;
use jp_tombmap::{Mut, TombMap};
use state::{LocalState, State, UserState};
use tracing::{debug, info, trace, warn};

const APPLICATION: &str = "jp";

#[derive(Debug)]
pub struct Workspace {
    /// The root directory of the workspace.
    ///
    /// This differs from the storage's root directory.
    root: PathBuf,

    /// The globally unique ID of the workspace.
    id: id::Id,

    /// The (optional) storage for the workspace.
    ///
    /// If this is `None`, the workspace is in-memory only.
    storage: Option<Storage>,

    /// The in-memory state of the workspace.
    ///
    /// If `storage` is `Some`, this is a copy of the persisted state. Any
    /// changes made during the lifetime of the workspace will be persisted
    /// atomically when `persist` is called.
    state: State,

    /// Disable persistence for the workspace, even if the workspace has a
    /// storage attached.
    ///
    /// This is useful when you want to force persistence to be disabled at
    /// runtime, for example when an unexpected situation occurs.
    disable_persistence: bool,
}

impl Workspace {
    /// Find the [`Workspace`] root by walking up the directory tree.
    #[must_use]
    pub fn find_root(mut current_dir: PathBuf, storage_dir: &str) -> Option<PathBuf> {
        if storage_dir.is_empty() {
            return None;
        }

        loop {
            let config_path = current_dir.join(storage_dir);
            if config_path.is_dir() {
                return Some(current_dir);
            }

            if !current_dir.pop() {
                return None;
            }
        }
    }

    /// Creates a new workspace with the given root directory.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self::new_with_id(root, id::Id::new())
    }

    /// Creates a new workspace with the given root directory and ID.
    pub fn new_with_id(root: impl AsRef<Path>, id: id::Id) -> Self {
        let root = root.as_ref().to_path_buf();
        trace!(root = %root.display(), id = %id, "Initializing Workspace.");

        Self {
            root,
            id,
            storage: None,
            state: State::default(),
            disable_persistence: false,
        }
    }

    /// Get the root path of the workspace.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Enable persistence for the workspace at the given (absolute) path.
    pub fn persisted_at(mut self, path: &Path) -> Result<Self> {
        trace!(path = %path.display(), "Enabling workspace persistence.");

        self.disable_persistence = false;
        self.storage = Some(Storage::new(path)?);
        Ok(self)
    }

    /// Enable local storage for the workspace.
    pub fn with_local_storage(mut self) -> Result<Self> {
        if self.storage.is_none() {
            return Err(Error::MissingStorage);
        }

        let root = user_data_dir()?.join("workspace");
        let id: &str = &self.id;
        let name = self
            .root
            .file_name()
            .ok_or_else(|| Error::NotDir(self.root.clone()))?
            .to_string_lossy();

        self.storage = self
            .storage
            .map(|storage| storage.with_user_storage(&root, name, id))
            .transpose()?;

        Ok(self)
    }

    /// Disable persistence for the workspace.
    ///
    /// If this is called, then [`Self::persist`] becomes a no-op.
    ///
    /// Persistence can be re-enabled by calling [`Self::persisted_at`].
    pub fn disable_persistence(&mut self) {
        self.disable_persistence = true;
    }

    /// Returns the path to the storage directory, if persistence is enabled.
    #[must_use]
    pub fn storage_path(&self) -> Option<&Path> {
        self.storage.as_ref().map(Storage::path)
    }

    /// Returns the path to the user storage directory, if persistence is
    /// enabled, and user storage is configured.
    #[must_use]
    pub fn user_storage_path(&self) -> Option<&Path> {
        self.storage.as_ref().and_then(Storage::user_storage_path)
    }

    /// Load the workspace state from the persisted storage.
    ///
    /// If the workspace is not persisted, this method will return an error.
    pub fn load(&mut self) -> Result<()> {
        trace!("Loading state.");

        let storage = self.storage.as_mut().ok_or(Error::MissingStorage)?;

        // Local state
        let mut metadata = storage.load_conversations_metadata()?;
        debug!(
            active_conversation_id = %metadata.active_conversation_id,
            "Loaded workspace state metadata."
        );

        let conversation_ids = storage.load_all_conversation_ids();
        let active_conversation = match storage
            .load_conversation_metadata(&metadata.active_conversation_id)
        {
            Ok(conversation) => conversation,
            // If the active conversation cannot be found on disk, we try to
            // load the last known conversation on disk, and if that fails, we
            // return an error.
            Err(error @ jp_storage::Error::Conversation(jp_conversation::Error::UnknownId(_))) => {
                let last_conversation_id = conversation_ids.last().copied();
                warn!(
                    %error,
                    missing_id = %metadata.active_conversation_id,
                    new_id = %last_conversation_id.as_ref().map(ToString::to_string).unwrap_or_default(),
                    "Failed to load active conversation, falling back to last stored conversation."
                );

                metadata.active_conversation_id = last_conversation_id.ok_or(error)?;
                storage.load_conversation_metadata(&metadata.active_conversation_id)?
            }
            Err(error) => return Err(error.into()),
        };

        let conversations = conversation_ids
            .iter()
            .filter(|id| id != &&metadata.active_conversation_id)
            .map(|id| (*id, OnceCell::new()))
            .collect();

        let mut events: TombMap<_, _> = conversation_ids
            .into_iter()
            .map(|id| (id, OnceCell::new()))
            .collect();

        // We can `set` without checking if the cell is already initialized, as
        // we just initialized it above.
        let _err = events
            .entry(metadata.active_conversation_id)
            .or_default()
            .set(storage.load_conversation_events(&metadata.active_conversation_id)?);

        self.state = State {
            local: LocalState {
                active_conversation,
                conversations,
                events,
            },
            user: UserState {
                conversations_metadata: metadata,
            },
        };

        Ok(())
    }

    /// Persists the current in-memory workspace state back to disk atomically.
    pub fn persist(&mut self) -> Result<()> {
        if self.disable_persistence {
            trace!("Persistence disabled, skipping.");
            return Ok(());
        }

        trace!("Persisting state.");

        let active_id = self.active_conversation_id();
        let storage = self.storage.as_mut().ok_or(Error::MissingStorage)?;

        storage.persist_conversations_metadata(&self.state.user.conversations_metadata)?;
        storage.persist_conversations_and_events(
            &self.state.local.conversations,
            &self.state.local.events,
            &self
                .state
                .user
                .conversations_metadata
                .active_conversation_id,
            &self.state.local.active_conversation,
        )?;
        storage.remove_ephemeral_conversations(&[active_id]);

        info!(path = %self.root.display(), "Persisted state.");
        Ok(())
    }

    /// Gets the ID of the active conversation.
    #[must_use]
    pub fn active_conversation_id(&self) -> ConversationId {
        self.state
            .user
            .conversations_metadata
            .active_conversation_id
    }

    /// Sets the active conversation ID (in memory).
    pub fn set_active_conversation_id(&mut self, id: ConversationId) -> Result<()> {
        // Remove the new active conversation from the list of conversations,
        // returning an error if it doesn't exist.
        let new_active_conversation = self
            .state
            .local
            .conversations
            .remove(&id)
            .and_then(|mut v| v.take())
            .ok_or(Error::not_found("Conversation", &id))?;

        // Replace the active conversation with the new one.
        let old_active_conversation = std::mem::replace(
            &mut self.state.local.active_conversation,
            new_active_conversation,
        );

        // Replace the active conversation ID with the new one.
        let old_active_conversation_id = std::mem::replace(
            &mut self
                .state
                .user
                .conversations_metadata
                .active_conversation_id,
            id,
        );

        // Insert the old active conversation back into the list of
        // conversations, but only if it has any events attached.
        if self
            .state
            .local
            .events
            .get(&old_active_conversation_id)
            .and_then(|v| v.get())
            .is_some_and(|v| !v.is_empty())
        {
            // Guaranteed to not be initialized.
            let _err = self
                .state
                .local
                .conversations
                .entry(old_active_conversation_id)
                .or_default()
                .set(old_active_conversation);
        }

        Ok(())
    }

    /// Returns an iterator over all conversations, including the active
    /// conversation.
    pub fn conversations(&self) -> impl Iterator<Item = (&ConversationId, &Conversation)> {
        iter::once((
            &self
                .state
                .user
                .conversations_metadata
                .active_conversation_id,
            &self.state.local.active_conversation,
        ))
        .chain(
            self.state
                .local
                .conversations
                .iter()
                .filter_map(|v| get_or_init_conversation(self.storage.as_ref(), v)),
        )
    }

    /// Returns an iterator over all mutable conversations, including the active
    /// conversation.
    ///
    /// This returns a [`jp_tombmap::Mut`] instead of a reference to the
    /// conversation, to allow for change tracking.
    pub fn conversations_mut(
        &mut self,
    ) -> impl Iterator<Item = (&ConversationId, Mut<'_, ConversationId, Conversation>)> {
        iter::once((
            &self
                .state
                .user
                .conversations_metadata
                .active_conversation_id,
            Mut::new_untracked(
                &self
                    .state
                    .user
                    .conversations_metadata
                    .active_conversation_id,
                &mut self.state.local.active_conversation,
            ),
        ))
        .chain(self.state.local.conversations.iter_mut().filter_map(
            |(id, conversation)| {
                maybe_init_conversation(self.storage.as_ref(), (id, &conversation));
                conversation.and_then(OnceCell::get_mut).map(|v| (id, v))
            },
        ))
    }

    /// Gets a reference to a conversation by its ID.
    #[must_use]
    pub fn get_conversation(&self, id: &ConversationId) -> Option<&Conversation> {
        self.conversations()
            .find_map(|(i, v)| (i == id).then_some(v))
    }

    /// Similar to [`Self::get_conversation`], but returns an error if the
    /// conversation does not exist.
    pub fn try_get_conversation(&self, id: &ConversationId) -> Result<&Conversation> {
        self.get_conversation(id)
            .ok_or_else(|| Error::NotFound("Conversation", id.to_string()))
    }

    /// Gets a mutable reference to a conversation by its ID.
    #[must_use]
    pub fn get_conversation_mut(
        &mut self,
        id: &ConversationId,
    ) -> Option<Mut<'_, ConversationId, Conversation>> {
        self.conversations_mut()
            .find_map(|(i, v)| (i == id).then_some(v))
    }

    /// Similar to [`Self::get_conversation_mut`], but returns an error if the
    /// conversation does not exist.
    pub fn try_get_conversation_mut(
        &mut self,
        id: &ConversationId,
    ) -> Result<Mut<'_, ConversationId, Conversation>> {
        self.get_conversation_mut(id)
            .ok_or_else(|| Error::NotFound("Conversation", id.to_string()))
    }

    /// Creates a new conversation.
    pub fn create_conversation(
        &mut self,
        conversation: Conversation,
        config: Arc<AppConfig>,
    ) -> ConversationId {
        let id = ConversationId::default();

        // This can only fail if `ConversationId::default()` is called multiple
        // times within the same nanosecond, which is highly unlikely, and not
        // an issue if it does happen.
        let _err = self
            .state
            .local
            .conversations
            .entry(id)
            .insert_entry(OnceCell::new())
            .get_mut()
            .set(conversation);

        // See above.
        let _err = self
            .state
            .local
            .events
            .entry(id)
            .insert_entry(OnceCell::new())
            .get_mut()
            .set(ConversationStream::new(config));
        id
    }

    /// Remove a conversation by its ID.
    ///
    /// This cannot remove the active conversation. If the active conversation
    /// needs to be removed, mark another conversation as active first.
    pub fn remove_conversation(&mut self, id: &ConversationId) -> Result<Option<Conversation>> {
        let active_id = self.active_conversation_id();
        if id == &active_id {
            return Err(Error::CannotRemoveActiveConversation(active_id));
        }

        // Make sure to load the conversation from disk first, so that our
        // `TombMap` can record the removal (allowing our persistence logic to
        // trigger a file deletion).
        if self.get_conversation(id).is_none() {
            return Ok(None);
        }

        Ok(self
            .state
            .local
            .conversations
            .remove(id)
            .and_then(|mut v| v.take()))
    }

    /// Gets a reference to the currently active conversation.
    ///
    /// Creates a new conversation if none exists.
    #[must_use]
    pub fn get_active_conversation(&self) -> &Conversation {
        &self.state.local.active_conversation
    }

    /// Gets a mutable reference to the currently active conversation.
    #[must_use]
    pub fn get_active_conversation_mut(&mut self) -> &mut Conversation {
        &mut self.state.local.active_conversation
    }

    /// Gets the event stream for a specific conversation.
    #[must_use]
    pub fn get_events(&self, id: &ConversationId) -> Option<&ConversationStream> {
        self.state
            .local
            .events
            .get_key_value(id)
            .and_then(|v| get_or_init_events(self.storage.as_ref(), v))
            .map(|v| v.1)
    }

    /// Similar to [`Self::get_events`], but returns an error if the
    /// conversation does not exist.
    pub fn try_get_events(&self, id: &ConversationId) -> Result<&ConversationStream> {
        self.get_events(id)
            .ok_or_else(|| Error::NotFound("Conversation", id.to_string()))
    }

    /// Gets a mutable reference to the event stream for a specific conversation.
    #[must_use]
    pub fn get_events_mut<'a>(
        &'a mut self,
        id: &'a ConversationId,
    ) -> Option<&'a mut ConversationStream> {
        self.state
            .local
            .events
            .get_mut(id)
            .and_then(|v| get_or_init_events_mut(self.storage.as_ref(), (id, v)))
            .map(|v| v.1)
    }

    /// Similar to [`Self::get_events_mut`], but returns an error if the
    /// conversation does not exist.
    pub fn try_get_events_mut<'a>(
        &'a mut self,
        id: &'a ConversationId,
    ) -> Result<&'a mut ConversationStream> {
        self.get_events_mut(id)
            .ok_or_else(|| Error::NotFound("Conversation", id.to_string()))
    }

    /// Returns the globally unique ID of the workspace.
    #[must_use]
    pub fn id(&self) -> &Id {
        &self.id
    }
}

fn get_or_init_events<'a>(
    storage: Option<&Storage>,
    (id, conversation): (&'a ConversationId, &'a OnceCell<ConversationStream>),
) -> Option<(&'a ConversationId, &'a ConversationStream)> {
    maybe_init_events(storage, (id, conversation));
    conversation.get().map(|v| (id, v))
}

fn get_or_init_events_mut<'a>(
    storage: Option<&Storage>,
    (id, conversation): (&'a ConversationId, &'a mut OnceCell<ConversationStream>),
) -> Option<(&'a ConversationId, &'a mut ConversationStream)> {
    maybe_init_events(storage, (id, conversation));
    conversation.get_mut().map(|v| (id, v))
}

fn get_or_init_conversation<'a>(
    storage: Option<&Storage>,
    (id, conversation): (&'a ConversationId, &'a OnceCell<Conversation>),
) -> Option<(&'a ConversationId, &'a Conversation)> {
    maybe_init_conversation(storage, (id, conversation));
    conversation.get().map(|v| (id, v))
}

fn maybe_init_conversation<'a>(
    storage: Option<&Storage>,
    (id, conversation): (&'a ConversationId, &'a OnceCell<Conversation>),
) {
    let Some(storage) = storage else {
        return;
    };

    if conversation.get().is_none() {
        let Ok(stream) = storage.load_conversation_metadata(id) else {
            warn!(%id, "Failed to load conversation metadata. Skipping.");
            return;
        };

        if let Err(error) = conversation.set(stream) {
            warn!(%id, ?error, "Failed to initialize conversation metadata. Skipping.");
        }
    }
}

fn maybe_init_events<'a>(
    storage: Option<&Storage>,
    (id, conversation): (&'a ConversationId, &'a OnceCell<ConversationStream>),
) {
    let Some(storage) = storage else {
        return;
    };

    if conversation.get().is_none() {
        let Ok(stream) = storage.load_conversation_events(id) else {
            warn!(%id, "Failed to load conversation events. Skipping.");
            return;
        };

        if let Err(error) = conversation.set(stream) {
            warn!(%id, ?error, "Failed to initialize conversation events. Skipping.");
        }
    }
}

pub fn user_data_dir() -> Result<PathBuf> {
    Ok(directories::ProjectDirs::from("", "", APPLICATION)
        .ok_or(Error::MissingHome)?
        .data_local_dir()
        .to_path_buf())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, time::Duration};

    use jp_config::{
        Config as _, PartialAppConfig,
        conversation::tool::RunMode,
        model::id::{PartialModelIdConfig, ProviderId},
    };
    use jp_storage::{CONVERSATIONS_DIR, METADATA_FILE, value::read_json};
    use tempfile::tempdir;
    use test_log::test;
    use time::UtcDateTime;

    use super::*;

    #[test]
    fn test_workspace_find_root() {
        struct TestCase {
            workspace_dir: &'static str,
            workspace_dir_name: Option<&'static str>,
            workspace_dir_name_is_file: bool,
            cwd: &'static str,
            expected: Option<&'static str>,
        }

        let workspace_dir_name = Some("test_workspace");
        let workspace_dir_name_is_file = false;

        let test_cases = HashMap::from([
            ("workspace in current directory", TestCase {
                workspace_dir: "project",
                workspace_dir_name,
                workspace_dir_name_is_file,
                cwd: "project",
                expected: Some("project"),
            }),
            ("workspace in parent directory", TestCase {
                workspace_dir: "project",
                workspace_dir_name,
                workspace_dir_name_is_file,
                cwd: "project/subdir",
                expected: Some("project"),
            }),
            ("workspace in grandparent directory", TestCase {
                workspace_dir: "project",
                workspace_dir_name,
                workspace_dir_name_is_file,
                cwd: "project/subdir/subsubdir",
                expected: Some("project"),
            }),
            ("no workspace directory", TestCase {
                workspace_dir: "project",
                workspace_dir_name: None,
                workspace_dir_name_is_file,
                cwd: "project",
                expected: None,
            }),
            ("workspace name is a file", TestCase {
                workspace_dir: "project",
                workspace_dir_name,
                workspace_dir_name_is_file: true,
                cwd: "project",
                expected: None,
            }),
            ("different workspace name", TestCase {
                workspace_dir: "project",
                workspace_dir_name: Some("different_name"),
                workspace_dir_name_is_file,
                cwd: "project",
                expected: None,
            }),
            ("empty workspace name", TestCase {
                workspace_dir: "project",
                workspace_dir_name: Some(""),
                workspace_dir_name_is_file,
                cwd: "project",
                expected: None,
            }),
        ]);

        for (name, case) in test_cases {
            #[allow(clippy::unnecessary_literal_unwrap)]
            let workspace_dir_name = workspace_dir_name.unwrap();

            let root = tempdir().unwrap().path().to_path_buf();
            let cwd = root.join(case.cwd);
            let project = root.join(case.workspace_dir);
            let expected = case.expected.map(|v| root.join(v));

            fs::create_dir_all(&cwd).unwrap();
            fs::create_dir_all(&project).unwrap();

            if case.workspace_dir_name.is_some() {
                if case.workspace_dir_name_is_file {
                    fs::write(project.join(workspace_dir_name), "").unwrap();
                } else {
                    fs::create_dir_all(project.join(workspace_dir_name)).unwrap();
                }
            }

            let result = Workspace::find_root(cwd, case.workspace_dir_name.unwrap_or("default"));
            assert_eq!(result, expected, "Failed test case: {name}");
        }
    }

    #[test]
    fn test_workspace_persist_saves_in_memory_state() {
        jp_id::global::set("foo".to_owned());

        let tmp = tempdir().unwrap();
        let root = tmp.path().join("root");
        let storage = root.join("storage");

        let mut workspace = Workspace::new(&root);

        let mut partial = PartialAppConfig::empty();
        partial.conversation.tools.defaults.run = Some(RunMode::Ask);
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Anthropic),
            name: Some("test".parse().unwrap()),
        }
        .into();

        let id = workspace.create_conversation(
            Conversation::default(),
            AppConfig::from_partial(partial).unwrap().into(),
        );
        workspace.set_active_conversation_id(id).unwrap();
        assert!(!storage.exists());

        assert_eq!(workspace.persist(), Err(Error::MissingStorage));

        let mut workspace = workspace.persisted_at(&storage).unwrap();
        workspace.persist().unwrap();
        assert!(storage.is_dir());

        let conversation_id = workspace.conversations().next().unwrap().0;
        let metadata_file = storage
            .join(CONVERSATIONS_DIR)
            .join(conversation_id.to_dirname(None))
            .join(METADATA_FILE);

        assert!(metadata_file.is_file());

        let _metadata: Conversation = read_json(&metadata_file).unwrap();
    }

    #[test]
    fn test_workspace_conversations() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert_eq!(workspace.conversations().count(), 1); // Default conversation

        let id = ConversationId::default();
        let conversation = Conversation::default();
        workspace
            .state
            .local
            .conversations
            .entry(id)
            .or_default()
            .set(conversation)
            .unwrap();
        assert_eq!(workspace.conversations().count(), 2);
    }

    #[test]
    fn test_workspace_get_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.local.conversations.is_empty());

        let id = ConversationId::try_from(UtcDateTime::now() - Duration::from_secs(1)).unwrap();
        assert_eq!(workspace.get_conversation(&id), None);

        let conversation = Conversation::default();
        workspace
            .state
            .local
            .conversations
            .entry(id)
            .or_default()
            .set(conversation.clone())
            .unwrap();
        assert_eq!(workspace.get_conversation(&id), Some(&conversation));
    }

    #[test]
    fn test_workspace_create_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.local.conversations.is_empty());

        let conversation = Conversation::default();
        let mut partial = PartialAppConfig::empty();
        partial.conversation.tools.defaults.run = Some(RunMode::Ask);
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Anthropic),
            name: Some("test".parse().unwrap()),
        }
        .into();

        let id = workspace.create_conversation(
            conversation.clone(),
            AppConfig::from_partial(partial).unwrap().into(),
        );
        assert_eq!(
            workspace
                .state
                .local
                .conversations
                .get(&id)
                .and_then(|v| v.get()),
            Some(&conversation)
        );
    }

    #[test]
    fn test_workspace_remove_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.local.conversations.is_empty());

        let id = ConversationId::try_from(UtcDateTime::now() - Duration::from_secs(1)).unwrap();
        let conversation = Conversation::default();
        workspace
            .state
            .local
            .conversations
            .entry(id)
            .or_default()
            .set(conversation.clone())
            .unwrap();

        assert_ne!(workspace.active_conversation_id(), id);
        let removed_conversation = workspace.remove_conversation(&id).unwrap().unwrap();
        assert_eq!(removed_conversation, conversation);
        assert!(workspace.state.local.conversations.is_empty());
    }

    #[test]
    fn test_workspace_cannot_remove_active_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.local.conversations.is_empty());

        let active_id = workspace
            .state
            .user
            .conversations_metadata
            .active_conversation_id;
        let active_conversation = workspace.state.local.active_conversation.clone();

        assert!(workspace.remove_conversation(&active_id).is_err());
        assert_eq!(
            workspace.state.local.active_conversation,
            active_conversation
        );
    }
}
