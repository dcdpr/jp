//! JP Workspace: A crate for managing LLM-assisted code conversations
//!
//! This crate provides data models and storage operations for the JP workspace,
//! a CLI tool for managing LLM-assisted code conversations with fine-grained
//! control over context and behavior.
//!
//! # Core Concepts
//!
//! - [`Workspace`]: Top-level container for all JP data
//! - [`Persona`]: Configuration specifying how the LLM should behave
//! - [`Conversation`]: A sequence of messages between user and LLM
//! - [`Attachment`]: Reference to contextual information for the LLM
//! - [`Message`]: Single exchange between user and LLM
//!
//! # Usage Example
//!
//! ```ignore
//! use std::path::PathBuf;
//!
//! use jp_workspace::{Attachment, Context, Workspace};
//!
//! // Initialize a workspace
//! let workspace = Workspace::new(PathBuf::from(".jp"));
//! workspace.init().expect("Failed to initialize workspace");
//!
//! // Create a new conversation
//! let attachments = vec![Attachment::File {
//!     includes: vec!["src/**/*.rs".to_string()],
//!     excludes: vec!["src/**/*.generated.rs".to_string()],
//! }];
//!
//! let context = Context {
//!     persona: "software-developer".into(),
//!     attachments,
//! };
//!
//! let conversation = workspace
//!     .create_conversation(Some("Rust code review".to_string()), context)
//!     .expect("Failed to create conversation");
//!
//! println!("Created conversation: {}", conversation.id);
//! ```

mod error;
pub mod id;
mod map;
mod state;
mod storage;

use std::{
    iter,
    path::{Path, PathBuf},
};

pub use error::Error;
use error::Result;
use jp_conversation::{
    Context, ContextId, Conversation, ConversationId, MessagePair, Model, ModelId, ModelReference,
    Persona, PersonaId,
};
use jp_mcp::config::{McpServer, McpServerId};
use state::{LocalState, State, WorkspaceState};
use storage::{Storage, DEFAULT_STORAGE_DIR};
use tracing::{debug, info, trace};

const APPLICATION: &str = "jp";

#[derive(Debug)]
pub struct Workspace {
    /// The root directory of the workspace.
    ///
    /// This differs from the storage's root directory.
    pub root: PathBuf,

    /// The globally unique ID of the workspace.
    id: String,

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
        Self::new_with_id(root, id::new())
    }

    /// Creates a new workspace with the given root directory and ID.
    pub fn new_with_id(root: impl AsRef<Path>, id: impl Into<String>) -> Self {
        let id = id.into();
        let root = root.as_ref().to_path_buf();
        trace!(root = %root.display(), id, "Initializing Workspace.");

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

        // Create unique local storage path based on (hashed) workspace path.
        let local = directories::ProjectDirs::from("", "", APPLICATION)
            .ok_or(Error::MissingHome)?
            .data_local_dir()
            .join(format!(
                "{}-{}",
                self.root
                    .file_name()
                    .ok_or_else(|| Error::NotDir(self.root.clone()))?
                    .to_string_lossy(),
                &self.id,
            ));

        self.storage = Some(storage.with_local(local)?);
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

    /// Load the workspace state from the persisted storage.
    ///
    /// If the workspace is not persisted, this method will return an error.
    pub fn load(&mut self) -> Result<()> {
        trace!("Loading state.");

        let storage = self.storage.as_mut().ok_or(Error::MissingStorage)?;

        // Workspace state
        let personas = storage.load_personas()?;
        let models = storage.load_models()?;
        let mcp_servers = storage.load_mcp_servers()?;
        let named_contexts = storage.load_named_contexts()?;
        let (mut conversations, messages) = storage.load_conversations_and_messages()?;

        // Local state
        let conversations_metadata = storage.load_conversations_metadata()?;

        debug!(
            contexts = %named_contexts.len(),
            conversations = %conversations.len(),
            personas = %personas.len(),
            models = %models.len(),
            mcp_servers = %mcp_servers.len(),
            active_conversation_id = %conversations_metadata.active_conversation_id,
            "Loaded workspace state."
        );

        // Remove the active conversation from the list of conversations, we
        // store it separately to ensure an active conversation always exists,
        // and cannot be removed.
        let active_conversation = conversations
            .remove(&conversations_metadata.active_conversation_id)
            .unwrap_or_else(|| {
                info!(
                    id = %conversations_metadata.active_conversation_id,
                    "Active conversation not found in workspace. Creating a new one."
                );

                Conversation::default()
            });

        self.state = State {
            workspace: WorkspaceState {
                active_conversation,
                named_contexts,
                conversations,
                messages,
                personas,
                models,
                mcp_servers,
            },
            local: LocalState {
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

        self.storage
            .as_mut()
            .ok_or(Error::MissingStorage)?
            .persist(&self.state)
    }

    /// Gets the ID of the active conversation.
    #[must_use]
    pub fn active_conversation_id(&self) -> ConversationId {
        self.state
            .local
            .conversations_metadata
            .active_conversation_id
    }

    /// Sets the active conversation ID (in memory).
    pub fn set_active_conversation_id(&mut self, id: ConversationId) -> Result<()> {
        // Remove the new active conversation from the list of conversations,
        // returning an error if it doesn't exist.
        let new_active_conversation = self
            .state
            .workspace
            .conversations
            .remove(&id)
            .ok_or(Error::not_found("Conversation", &id))?;

        // Replace the active conversation with the new one.
        let old_active_conversation = std::mem::replace(
            &mut self.state.workspace.active_conversation,
            new_active_conversation,
        );

        // Replace the active conversation ID with the new one.
        let old_active_conversation_id = std::mem::replace(
            &mut self
                .state
                .local
                .conversations_metadata
                .active_conversation_id,
            id,
        );

        // Insert the old active conversation back into the list of
        // conversations.
        self.state
            .workspace
            .conversations
            .insert(old_active_conversation_id, old_active_conversation);

        Ok(())
    }

    /// Returns an iterator over all personas.
    pub fn personas(&self) -> impl Iterator<Item = (&PersonaId, &Persona)> {
        self.state.workspace.personas.iter()
    }

    /// Gets a reference to a persona by its ID.
    #[must_use]
    pub fn get_persona(&self, id: &PersonaId) -> Option<&Persona> {
        self.state.workspace.personas.get(id)
    }

    /// Create a new persona.
    ///
    /// Returns an error if a persona with that ID already exists.
    pub fn create_persona(&mut self, persona: Persona) -> Result<PersonaId> {
        let id = PersonaId::try_from(&persona.name)?;
        self.create_persona_with_id(id, persona)
    }

    /// Create a new persona with the given ID.
    ///
    /// Returns an error if a persona with that ID already exists.
    pub fn create_persona_with_id(&mut self, id: PersonaId, persona: Persona) -> Result<PersonaId> {
        use map::Entry::*;

        let id = match self.state.workspace.personas.entry(id) {
            Occupied(entry) => return Err(Error::exists("Persona", entry.key())),
            Vacant(entry) => entry.insert_entry(persona).key().clone(),
        };

        Ok(id)
    }

    /// Removes a persona by its ID.
    ///
    /// Returns the removed persona if it existed.
    pub fn remove_persona(&mut self, id: &PersonaId) -> Option<Persona> {
        if id == &PersonaId::default() {
            return None;
        }

        self.state.workspace.personas.remove(id)
    }

    /// Returns an iterator over all defined LLM models.
    pub fn models(&self) -> impl Iterator<Item = (&ModelId, &Model)> {
        self.state.workspace.models.iter()
    }

    /// Gets a reference to an LLM model by its ID.
    #[must_use]
    pub fn get_model(&self, id: &ModelId) -> Option<&Model> {
        self.state.workspace.models.get(id)
    }

    /// Resolves an `LlmModelReference` to a concrete `LlmModel`.
    /// Returns `None` if the reference is an ID that doesn't exist.
    pub fn resolve_model_reference<'a>(
        &'a self,
        reference: &'a ModelReference,
    ) -> Result<&'a Model> {
        match reference {
            ModelReference::Inline(model) => Ok(model),
            ModelReference::Ref(id) => self.get_model(id).ok_or(Error::not_found("Model", id)),
        }
    }

    /// Creates a new model.
    ///
    /// Returns an error if a model with that ID already exists.
    pub fn create_model(&mut self, model: Model) -> Result<ModelId> {
        let id = ModelId::try_from((model.provider, model.slug.as_str()))?;
        self.create_model_with_id(id, model)
    }

    /// Creates a new model.
    ///
    /// Returns an error if a model with that ID already exists.
    pub fn create_model_with_id(&mut self, id: ModelId, model: Model) -> Result<ModelId> {
        if self.state.workspace.models.contains_key(&id) {
            return Err(Error::exists("Model", &id));
        }

        self.state.workspace.models.insert(id.clone(), model);
        Ok(id)
    }

    /// Removes an LLM model by its ID.
    ///
    /// Returns the removed model if it existed.
    pub fn remove_model(&mut self, id: &ModelId) -> Option<Model> {
        self.state.workspace.models.remove(id)
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
        self.all_conversations_mut()
            .find_map(|(i, c)| (id == i).then_some(c))
    }

    /// Creates a new conversation.
    pub fn create_conversation(&mut self, conversation: Conversation) -> ConversationId {
        let id = ConversationId::default();

        self.state.workspace.conversations.insert(id, conversation);
        self.state.workspace.messages.entry(id).or_default();
        id
    }

    /// Remove a conversation by its ID.
    ///
    /// This cannot remove the active conversation. If the active conversation
    /// needs to be removed, mark another conversation as active first.
    pub fn remove_conversation(&mut self, id: &ConversationId) -> Option<Conversation> {
        self.state.workspace.conversations.remove(id)
    }

    /// Gets a reference to the currently active conversation.
    ///
    /// Creates a new conversation if none exists.
    #[must_use]
    pub fn get_active_conversation(&self) -> &Conversation {
        &self.state.workspace.active_conversation
    }

    /// Gets a mutable reference to the currently active conversation.
    #[must_use]
    pub fn get_active_conversation_mut(&mut self) -> &mut Conversation {
        &mut self.state.workspace.active_conversation
    }

    /// Gets the messages for a specific conversation. Returns an empty slice if not found.
    #[must_use]
    pub fn get_messages(&self, id: &ConversationId) -> &[MessagePair] {
        self.state
            .workspace
            .messages
            .get(id)
            .map_or(&[], |v| v.as_slice())
    }

    /// Removes the last message from a conversation.
    pub fn pop_message(&mut self, id: &ConversationId) -> Option<MessagePair> {
        self.state.workspace.messages.get_mut(id).and_then(Vec::pop)
    }

    /// Adds a message to a conversation.
    pub fn add_message(&mut self, id: ConversationId, message: MessagePair) {
        self.state
            .workspace
            .messages
            .entry(id)
            .or_default()
            .push(message);
    }

    /// Returns an iterator over all configured MCP servers.
    pub fn mcp_servers(&self) -> impl Iterator<Item = &McpServer> {
        self.state.workspace.mcp_servers.values()
    }

    /// Gets a reference to an MCP server by its ID.
    #[must_use]
    pub fn get_mcp_server(&self, id: &McpServerId) -> Option<&McpServer> {
        self.state.workspace.mcp_servers.get(id)
    }

    /// Adds an MCP Server configuration.
    pub fn create_mcp_server(&mut self, server: McpServer) -> Option<McpServer> {
        let id = server.id.clone();
        self.state.workspace.mcp_servers.insert(id, server)
    }

    /// Removes an MCP server configuration by ID.
    pub fn remove_mcp_server(&mut self, id: &McpServerId) -> Option<McpServer> {
        self.state.workspace.mcp_servers.remove(id)
    }

    /// Returns an iterator over all named contexts.
    pub fn named_contexts(&self) -> impl Iterator<Item = (&ContextId, &Context)> {
        self.state.workspace.named_contexts.iter()
    }

    /// Gets a reference to a named context by its ID.
    #[must_use]
    pub fn get_named_context(&self, id: &ContextId) -> Option<&Context> {
        self.state.workspace.named_contexts.get(id)
    }

    /// Returns an iterator over all conversations, including the active one.
    fn all_conversations(&self) -> impl Iterator<Item = (&ConversationId, &Conversation)> {
        self.state.workspace.conversations.iter().chain(iter::once((
            &self
                .state
                .local
                .conversations_metadata
                .active_conversation_id,
            &self.state.workspace.active_conversation,
        )))
    }

    /// Returns an iterator over all conversations, including the active one.
    fn all_conversations_mut(
        &mut self,
    ) -> impl Iterator<Item = (&ConversationId, &mut Conversation)> {
        self.state
            .workspace
            .conversations
            .iter_mut()
            .chain(iter::once((
                &self
                    .state
                    .local
                    .conversations_metadata
                    .active_conversation_id,
                &mut self.state.workspace.active_conversation,
            )))
    }

    /// Returns the globally unique ID of the workspace.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, fs, time::Duration};

    use jp_conversation::model::ProviderId;
    use tempfile::tempdir;
    use time::UtcDateTime;

    use super::*;
    use crate::storage::{read_json, write_json, CONVERSATIONS_DIR, METADATA_FILE, PERSONAS_DIR};

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
    fn test_workspace_load_loads_persisted_state() {
        jp_id::global::set("foo".to_owned());

        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join("storage");

        let personas_path = storage.join(PERSONAS_DIR);
        fs::create_dir_all(&personas_path).unwrap();

        let id = PersonaId::try_from("p1").unwrap();
        let persona = Persona {
            name: "p1".into(),
            ..Default::default()
        };
        write_json(&personas_path.join(id.to_path_buf()), &persona).unwrap();

        let mut workspace = Workspace::new(root).persisted_at(&storage).unwrap();
        workspace.load().unwrap();

        assert_eq!(workspace.get_persona(&id), Some(&persona));
    }

    #[test]
    fn test_workspace_personas() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert_eq!(workspace.personas().count(), 0);

        let id = PersonaId::try_from("p1").unwrap();
        let persona = Persona::new("p1");
        workspace.state.workspace.personas.insert(id, persona);
        assert_eq!(workspace.personas().count(), 1);
    }

    #[test]
    fn test_workspace_get_persona() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        let id = PersonaId::try_from("p1").unwrap();
        assert_eq!(workspace.get_persona(&id), None);

        let persona = Persona::new("p1");
        workspace
            .state
            .workspace
            .personas
            .insert(id.clone(), persona.clone());
        assert_eq!(workspace.get_persona(&id), Some(&persona));
    }

    #[test]
    fn test_workspace_create_persona() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.personas.is_empty());

        let persona = Persona::new("p1");
        let id = workspace.create_persona(persona.clone()).unwrap();
        assert_eq!(workspace.get_persona(&id), Some(&persona));
    }

    #[test]
    fn test_workspace_create_persona_duplicate() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.personas.is_empty());

        let persona = Persona::new("p1");
        let id = workspace.create_persona(persona.clone()).unwrap();
        assert_eq!(workspace.state.workspace.personas.get(&id), Some(&persona));

        let error = workspace.create_persona(persona).unwrap_err();
        assert_eq!(error, Error::exists("Persona", &id));
        assert_eq!(workspace.state.workspace.personas.len(), 1);
    }

    #[test]
    fn test_workspace_remove_persona() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.personas.is_empty());

        let id = PersonaId::try_from("p1").unwrap();
        let persona = Persona::new("p1");
        workspace
            .state
            .workspace
            .personas
            .insert(id.clone(), persona.clone());

        let removed_persona = workspace.remove_persona(&id).unwrap();
        assert_eq!(removed_persona, persona);
        assert!(workspace.state.workspace.personas.is_empty());
    }

    #[test]
    fn test_workspace_remove_persona_ignores_default() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.personas.is_empty());

        let id = PersonaId::try_from("default").unwrap();
        let persona = Persona::default();
        workspace
            .state
            .workspace
            .personas
            .insert(id.clone(), persona.clone());

        let removed_persona = workspace.remove_persona(&id);
        assert!(removed_persona.is_none());
        assert_eq!(workspace.state.workspace.personas.len(), 1);
    }

    #[test]
    fn test_workspace_models() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert_eq!(workspace.models().count(), 0);

        let id = ModelId::try_from("openrouter/p1").unwrap();
        let model = Model::new(ProviderId::Openrouter, "p1");
        workspace.state.workspace.models.insert(id, model);
        assert_eq!(workspace.models().count(), 1);
    }

    #[test]
    fn test_workspace_get_model() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        let id = ModelId::try_from("openrouter/p1").unwrap();
        assert_eq!(workspace.get_model(&id), None);

        let model = Model::new(ProviderId::Openrouter, "p1");
        workspace
            .state
            .workspace
            .models
            .insert(id.clone(), model.clone());
        assert_eq!(workspace.get_model(&id), Some(&model));
    }

    #[test]
    fn test_workspace_create_model() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.models.is_empty());

        let model = Model::new(ProviderId::Openrouter, "p1");
        let id = workspace.create_model(model.clone()).unwrap();
        assert_eq!(workspace.state.workspace.models.get(&id), Some(&model));
    }

    #[test]
    fn test_workspace_create_model_duplicate() {
        jp_id::global::set("foo".to_owned());

        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.models.is_empty());

        let model = Model::new(ProviderId::Openrouter, "p1");
        let id = workspace.create_model(model.clone()).unwrap();
        assert_eq!(workspace.state.workspace.models.get(&id), Some(&model));

        let error = workspace.create_model(model).unwrap_err();
        assert_eq!(error, Error::exists("Model", &id));
        assert_eq!(workspace.state.workspace.models.len(), 1);
    }

    #[test]
    fn test_workspace_remove_model() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.models.is_empty());

        let id = ModelId::try_from("openrouter/p1").unwrap();
        let model = Model::new(ProviderId::Openrouter, "p1");
        workspace
            .state
            .workspace
            .models
            .insert(id.clone(), model.clone());

        let removed_model = workspace.remove_model(&id).unwrap();
        assert_eq!(removed_model, model);
        assert!(workspace.state.workspace.models.is_empty());
    }

    #[test]
    fn test_workspace_conversations() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert_eq!(workspace.conversations().count(), 1); // Default conversation

        let id = ConversationId::default();
        let conversation = Conversation::default();
        workspace
            .state
            .workspace
            .conversations
            .insert(id, conversation);
        assert_eq!(workspace.conversations().count(), 2);
    }

    #[test]
    fn test_workspace_get_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.conversations.is_empty());

        let id = ConversationId::try_from(UtcDateTime::now() - Duration::from_secs(1)).unwrap();
        assert_eq!(workspace.get_conversation(&id), None);

        let conversation = Conversation::default();
        workspace
            .state
            .workspace
            .conversations
            .insert(id, conversation.clone());
        assert_eq!(workspace.get_conversation(&id), Some(&conversation));
    }

    #[test]
    fn test_workspace_create_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.conversations.is_empty());

        let conversation = Conversation::default();
        let id = workspace.create_conversation(conversation.clone());
        assert_eq!(
            workspace.state.workspace.conversations.get(&id),
            Some(&conversation)
        );
    }

    #[test]
    fn test_workspace_remove_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.conversations.is_empty());

        let id = ConversationId::try_from(UtcDateTime::now() - Duration::from_secs(1)).unwrap();
        let conversation = Conversation::default();
        workspace
            .state
            .workspace
            .conversations
            .insert(id, conversation.clone());

        assert_ne!(workspace.active_conversation_id(), id);
        let removed_conversation = workspace.remove_conversation(&id).unwrap().unwrap();
        assert_eq!(removed_conversation, conversation);
        assert!(workspace.state.workspace.conversations.is_empty());
    }

    #[test]
    fn test_workspace_cannot_remove_active_conversation() {
        let mut workspace = Workspace::new(PathBuf::new());
        assert!(workspace.state.workspace.conversations.is_empty());

        let active_id = workspace
            .state
            .local
            .conversations_metadata
            .active_conversation_id;
        let active_conversation = workspace.state.workspace.active_conversation.clone();

        assert!(workspace.remove_conversation(&active_id).is_err());
        assert_eq!(
            workspace.state.workspace.active_conversation,
            active_conversation
        );
    }
}
