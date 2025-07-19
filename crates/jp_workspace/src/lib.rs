//! JP Workspace: A crate for managing LLM-assisted code conversations
//!
//! This crate provides data models and storage operations for the JP workspace,
//! a CLI tool for managing LLM-assisted code conversations with fine-grained
//! control over context and behavior.

mod error;
mod id;
pub mod query;
mod state;

use std::{
    iter,
    path::{Path, PathBuf},
};

pub use error::Error;
use error::Result;
pub use id::Id;
use jp_conversation::{Conversation, ConversationId, MessagePair};
use jp_mcp::{
    config::{McpServer, McpServerId},
    tool::McpTool,
};
use jp_storage::{Storage, DEFAULT_STORAGE_DIR, MCP_SERVERS_DIR};
use state::{LocalState, State, UserState};
use tracing::{debug, info, trace};

const APPLICATION: &str = "jp";

#[derive(Debug)]
pub struct Workspace {
    /// The root directory of the workspace.
    ///
    /// This differs from the storage's root directory.
    pub root: PathBuf,

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

    /// Enable persistence for the workspace.
    ///
    /// The workspace will be persisted to the default storage directory in the
    /// workspace root, which is `.jp/`.
    ///
    /// See also: [`Self::persisted_at`].
    pub fn persisted(self) -> Result<Self> {
        let path = self.root.join(DEFAULT_STORAGE_DIR);
        self.persisted_at(&path)
    }

    /// Enable persistence for the workspace at the given (absolute) path.
    ///
    /// See also: [`Self::persisted`].
    pub fn persisted_at(mut self, path: &Path) -> Result<Self> {
        trace!(path = %path.display(), "Enabling workspace persistence.");

        self.disable_persistence = false;
        self.storage = Some(Storage::new(path)?);
        Ok(self)
    }

    /// Enable local storage for the workspace.
    pub fn with_local_storage(mut self) -> Result<Self> {
        let storage = self.storage.take().ok_or(Error::MissingStorage)?;

        let id: &str = &self.id;
        let name = self
            .root
            .file_name()
            .ok_or_else(|| Error::NotDir(self.root.clone()))?
            .to_string_lossy();

        self.storage = Some(storage.with_user_storage(&user_data_dir()?, name, id)?);
        Ok(self)
    }

    /// Disable persistence for the workspace.
    ///
    /// If this is called, then [`Self::persist`] becomes a no-op.
    ///
    /// Persistence can be re-enabled by calling [`Self::persisted`].
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

        // Workspace state
        let mcp_servers = storage.load_mcp_servers()?;
        let mcp_tools = storage.load_mcp_tools()?;
        let (mut conversations, messages) = storage.load_conversations_and_messages()?;

        // Local state
        let conversations_metadata = storage.load_conversations_metadata()?;

        debug!(
            conversations = %conversations.len(),
            mcp_servers = %mcp_servers.len(),
            mcp_tools = %mcp_tools.len(),
            active_conversation_id = %conversations_metadata.active_conversation_id,
            "Loaded workspace state."
        );

        // Remove the active conversation from the list of conversations, we
        // store it separately to ensure an active conversation always exists,
        // and cannot be removed.
        let active_conversation = conversations
            .remove_untracked(&conversations_metadata.active_conversation_id)
            .unwrap_or_else(|| {
                info!(
                    id = %conversations_metadata.active_conversation_id,
                    "Active conversation not found in workspace. Creating a new one."
                );

                Conversation::default()
            });

        self.state = State {
            local: LocalState {
                active_conversation,
                conversations,
                messages,
                mcp_servers,
                mcp_tools,
            },
            user: UserState {
                conversations_metadata,
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

        let storage = self.storage.as_mut().ok_or(Error::MissingStorage)?;

        storage.persist_conversations_metadata(&self.state.user.conversations_metadata)?;
        storage.persist_conversations_and_messages(
            &self.state.local.conversations,
            &self.state.local.messages,
            &self
                .state
                .user
                .conversations_metadata
                .active_conversation_id,
            &self.state.local.active_conversation,
        )?;
        storage.persist_mcp_servers(&self.state.local.mcp_servers)?;

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
        // conversations, but only if it has any messages attached.
        if self
            .state
            .local
            .messages
            .get(&old_active_conversation_id)
            .is_some_and(|v| !v.is_empty())
        {
            self.state
                .local
                .conversations
                .insert(old_active_conversation_id, old_active_conversation);
        }

        Ok(())
    }

    /// Returns an iterator over all conversations.
    pub fn conversations(&self) -> impl Iterator<Item = (&ConversationId, &Conversation)> {
        self.all_conversations()
    }

    /// Gets a reference to a conversation by its ID.
    #[must_use]
    pub fn get_conversation(&self, id: &ConversationId) -> Option<&Conversation> {
        self.all_conversations()
            .find_map(|(i, c)| (id == i).then_some(c))
    }

    /// Gets a mutable reference to a conversation by its ID.
    #[must_use]
    pub fn get_conversation_mut(&mut self, id: &ConversationId) -> Option<&mut Conversation> {
        if id == &self.active_conversation_id() {
            return Some(&mut self.state.local.active_conversation);
        }

        self.state.local.conversations.get_mut(id)
    }

    /// Creates a new conversation.
    pub fn create_conversation(&mut self, conversation: Conversation) -> ConversationId {
        let id = ConversationId::default();

        self.state.local.conversations.insert(id, conversation);
        self.state.local.messages.entry(id).or_default();
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

        Ok(self.state.local.conversations.remove(id))
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

    /// Gets the messages for a specific conversation. Returns an empty slice if not found.
    #[must_use]
    pub fn get_messages(&self, id: &ConversationId) -> &[MessagePair] {
        self.state
            .local
            .messages
            .get(id)
            .map_or(&[], |v| v.as_slice())
    }

    /// Removes the last message from a conversation.
    pub fn pop_message(&mut self, id: &ConversationId) -> Option<MessagePair> {
        self.state.local.messages.get_mut(id).and_then(Vec::pop)
    }

    /// Adds a message to a conversation.
    pub fn add_message(&mut self, id: ConversationId, message: MessagePair) {
        self.state
            .local
            .messages
            .entry(id)
            .or_default()
            .push(message);
    }

    /// Returns an iterator over all configured MCP tools.
    pub fn mcp_tools(&self) -> impl Iterator<Item = &McpTool> {
        self.state.local.mcp_tools.values()
    }

    /// Returns an iterator over all configured MCP servers.
    pub fn mcp_servers(&self) -> impl Iterator<Item = &McpServer> {
        self.state.local.mcp_servers.values()
    }

    /// Returns the path to the MCP servers directory, if storage is enabled.
    #[must_use]
    pub fn mcp_servers_path(&self) -> Option<PathBuf> {
        self.storage
            .as_ref()
            .map(|p| p.path().join(MCP_SERVERS_DIR))
    }

    /// Returns the path to the local MCP servers directory, if storage is
    /// enabled, and local storage is configured.
    #[must_use]
    pub fn mcp_servers_local_path(&self) -> Option<PathBuf> {
        self.storage
            .as_ref()
            .and_then(|p| p.user_storage_path().map(|p| p.join(MCP_SERVERS_DIR)))
    }

    /// Gets a reference to an MCP server by its ID.
    #[must_use]
    pub fn get_mcp_server(&self, id: &McpServerId) -> Option<&McpServer> {
        self.state.local.mcp_servers.get(id)
    }

    /// Adds an MCP Server configuration.
    pub fn create_mcp_server(&mut self, server: McpServer) -> Option<McpServer> {
        let id = server.id.clone();
        self.state.local.mcp_servers.insert(id, server)
    }

    /// Removes an MCP server configuration by ID.
    pub fn remove_mcp_server(&mut self, id: &McpServerId) -> Option<McpServer> {
        self.state.local.mcp_servers.remove(id)
    }

    /// Returns an iterator over all conversations, including the active one.
    fn all_conversations(&self) -> impl Iterator<Item = (&ConversationId, &Conversation)> {
        self.state.local.conversations.iter().chain(iter::once((
            &self
                .state
                .user
                .conversations_metadata
                .active_conversation_id,
            &self.state.local.active_conversation,
        )))
    }

    /// Returns the globally unique ID of the workspace.
    #[must_use]
    pub fn id(&self) -> &Id {
        &self.id
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

    use jp_storage::{value::read_json, CONVERSATIONS_DIR, METADATA_FILE};
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

        let id = workspace.create_conversation(Conversation::default());
        workspace.set_active_conversation_id(id).unwrap();
        assert!(!storage.exists());

        assert_eq!(workspace.persist(), Err(Error::MissingStorage));

        let mut workspace = workspace.persisted_at(&storage).unwrap();
        workspace.persist().unwrap();
        assert!(storage.is_dir());

        let conversation_id = workspace.conversations().next().unwrap().0;
        let metadata_file = storage
            .join(CONVERSATIONS_DIR)
            .join(conversation_id.to_dirname(None).unwrap())
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
        workspace.state.local.conversations.insert(id, conversation);
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
            .insert(id, conversation.clone());
        assert_eq!(workspace.get_conversation(&id), Some(&conversation));
    }

    #[test]
    fn test_workspace_create_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.local.conversations.is_empty());

        let conversation = Conversation::default();
        let id = workspace.create_conversation(conversation.clone());
        assert_eq!(
            workspace.state.local.conversations.get(&id),
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
            .insert(id, conversation.clone());

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
