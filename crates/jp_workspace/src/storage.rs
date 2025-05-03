//! Handles the physical storage aspects, including temporary copying and persistence.

use std::{
    fs,
    io::{BufReader, BufWriter, Write as _},
    iter,
    path::{Path, PathBuf},
    str::FromStr as _,
};

use jp_conversation::{
    model::ProviderId, Context, ContextId, Conversation, ConversationId, MessagePair, Model,
    ModelId, Persona, PersonaId,
};
use jp_id::Id as _;
use jp_mcp::config::{McpServer, McpServerId};
use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, trace, warn};

use crate::{
    error::{Error, Result},
    map::TombMap,
    state::ConversationsMetadata,
    State,
};

type ConversationsAndMessages = (
    TombMap<ConversationId, Conversation>,
    TombMap<ConversationId, Vec<MessagePair>>,
);

pub const DEFAULT_STORAGE_DIR: &str = ".jp";
pub(crate) const METADATA_FILE: &str = "metadata.json";
const MESSAGES_FILE: &str = "messages.json";
const CONTEXTS_DIR: &str = "contexts";
pub(crate) const PERSONAS_DIR: &str = "personas";
pub(crate) const MODELS_DIR: &str = "models";
pub(crate) const CONVERSATIONS_DIR: &str = "conversations";
pub(crate) const MCP_SERVERS_DIR: &str = "mcp";

#[derive(Debug)]
pub(crate) struct Storage {
    /// The path to the original storage directory.
    root: PathBuf,

    /// The path to the local storage directory.
    ///
    /// This is used (among other things) to store the active conversation id.
    ///
    /// If unset, local storage is disabled.
    local: Option<PathBuf>,
}

impl Storage {
    /// Creates a new Storage instance by creating a temporary directory and
    /// copying the contents of `root` into it.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Result<Self> {
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

        Ok(Self { root, local: None })
    }

    pub fn with_local(mut self, local: impl Into<PathBuf>) -> Result<Self> {
        let local: PathBuf = local.into();

        // Create local storage directory, if needed.
        if local.exists() {
            if !local.is_dir() {
                return Err(Error::NotDir(local));
            }
        } else {
            fs::create_dir_all(&local)?;
            trace!(path = %local.display(), "Created local storage directory.");
        }

        // Create reference back to workspace storage.
        let link = local.join("storage");
        if link.exists() {
            if !link.is_symlink() {
                return Err(Error::NotSymlink(link));
            }
        } else {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&self.root, local.join("storage"))?;
            #[cfg(windows)]
            std::os::windows::fs::symlink_dir(&self.root, local.join("storage"))?;
        }

        self.local = Some(local);
        Ok(self)
    }

    /// Returns the path to the storage directory.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Loads the conversations metadata from storage.
    ///
    /// This loads the file from local storage if configured, otherwise the
    /// workspace storage is used.
    ///
    /// If the file does not exist, return default conversations metadata.
    pub(crate) fn load_conversations_metadata(&self) -> Result<ConversationsMetadata> {
        let root = self.local.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        trace!(path = %metadata_path.display(), "Loading local conversations metadata.");

        if !metadata_path.exists() {
            return Ok(ConversationsMetadata::default());
        }

        read_json(&metadata_path)
    }

    /// Loads all personas from the (copied) storage.
    pub(crate) fn load_personas(&self) -> Result<TombMap<PersonaId, Persona>> {
        let personas_path = self.root.join(PERSONAS_DIR);
        trace!(path = %personas_path.display(), "Loading personas.");

        let mut personas = TombMap::new();

        for entry in fs::read_dir(&personas_path).ok().into_iter().flatten() {
            let path = entry?.path();

            if !path.is_file() || path.extension().is_some_and(|ext| ext != "json") {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(id) = PersonaId::from_filename(filename) else {
                warn!(?path, "Invalid persona filename. Skipping.");
                continue;
            };
            let Ok(persona) = read_json::<Persona>(&path) else {
                warn!(?path, "Failed to read persona file. Skipping.");
                continue;
            };

            personas.insert(id, persona);
        }

        let default_id = PersonaId::try_from("default")?;
        if personas.is_empty() || !personas.contains_key(&default_id) {
            personas.insert(default_id, Persona::default());
        }

        Ok(personas)
    }

    /// Loads all LLM models from the (copied) storage.
    pub(crate) fn load_models(&self) -> Result<TombMap<ModelId, Model>> {
        let models_path = self.root.join(MODELS_DIR);
        trace!(path = %models_path.display(), "Loading models.");

        let mut models = TombMap::new();

        for entry in fs::read_dir(&models_path).ok().into_iter().flatten() {
            let path = entry?.path();

            if !path.is_dir() {
                continue;
            }
            let Some(provider) = path.file_name().and_then(|v| v.to_str()) else {
                warn!(?path, "Invalid model directory name. Skipping.");
                continue;
            };
            let Ok(provider) = ProviderId::from_str(provider) else {
                warn!(%provider, "Invalid model provider. Skipping.");
                continue;
            };

            for entry in fs::read_dir(&path).ok().into_iter().flatten() {
                let path = entry?.path();
                if !path.is_file() || path.extension().is_some_and(|ext| ext != "json") {
                    warn!(?path, "Invalid model file type. Skipping.");
                    continue;
                }
                let Some(id) = path.file_name().and_then(|v| v.to_str()) else {
                    warn!(?path, "Invalid model file name. Skipping.");
                    continue;
                };
                let Ok(mut model) = read_json::<Model>(&path) else {
                    warn!(?path, "Failed to read model file. Skipping.");
                    continue;
                };

                model.provider = provider;
                models.insert(ModelId::from_path(&format!("{provider}/{id}"))?, model);
            }
        }

        if models.is_empty() {
            models.insert(ModelId::default(), Model::default());
        }

        Ok(models)
    }

    /// Loads all MCP Servers from the (copied) storage.
    pub(crate) fn load_mcp_servers(&self) -> Result<TombMap<McpServerId, McpServer>> {
        let mcp_path = self.root.join(MCP_SERVERS_DIR);
        trace!(path = %mcp_path.display(), "Loading MCP servers.");

        let mut servers = TombMap::new();

        for entry in fs::read_dir(&mcp_path).ok().into_iter().flatten() {
            let path = entry?.path();
            if !path.is_file() || path.extension().is_some_and(|ext| ext != "json") {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(id_str) = filename.strip_suffix(".json") else {
                continue;
            };
            let Ok(mut server) = read_json::<McpServer>(&path) else {
                warn!(?path, "Failed to read MCP server file. Skipping.");
                continue;
            };

            let id = McpServerId::new(id_str);
            server.id = id.clone();
            servers.insert(id, server);
        }

        Ok(servers)
    }

    /// Loads all Named Contexts from the (copied) storage.
    pub(crate) fn load_named_contexts(&self) -> Result<TombMap<ContextId, Context>> {
        let contexts_path = self.root.join(CONTEXTS_DIR);
        trace!(path = %contexts_path.display(), "Loading named contexts.");

        let mut contexts = TombMap::new();

        for entry in fs::read_dir(&contexts_path).ok().into_iter().flatten() {
            let path = entry?.path();
            if !path.is_file() || path.extension().is_some_and(|ext| ext != "json") {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(id) = ContextId::from_filename(filename) else {
                warn!(?path, "Invalid context filename. Skipping.");
                continue;
            };
            let Ok(context) = read_json::<Context>(&path) else {
                warn!(?path, "Failed to read context file. Skipping.");
                continue;
            };

            contexts.insert(id, context);
        }

        Ok(contexts)
    }

    /// Loads all conversations and their associated messages, including
    /// private/local conversations.
    #[allow(clippy::type_complexity)]
    pub(crate) fn load_conversations_and_messages(&self) -> Result<ConversationsAndMessages> {
        let (mut conversations, mut messages) =
            load_conversations_and_messages_from_dir(&self.root)?;

        if let Some(local) = self.local.as_ref() {
            let (mut local_conversations, local_messages) =
                load_conversations_and_messages_from_dir(local)?;

            for (_, conversation) in local_conversations.iter_mut_untracked() {
                conversation.private = true;
            }

            conversations.extend(local_conversations);
            messages.extend(local_messages);
        }

        Ok((conversations, messages))
    }

    /// Persists the entire storage state to disk atomically.
    pub(crate) fn persist(&mut self, state: &State) -> Result<()> {
        trace!("Persisting state.");

        // Some data is stored locally, if configured, otherwise it is stored
        // in the workspace storage.
        let root_path = self.root.as_path();
        let local_or_root_path = self.local.as_deref().unwrap_or(root_path);

        // Step 1: Write state to local or temp dir
        persist_conversations_metadata(state, local_or_root_path)?;
        persist_personas(state, root_path)?;
        persist_models(state, root_path)?;
        persist_conversations_and_messages(state, root_path, local_or_root_path)?;
        persist_mcp_servers(state, root_path)?;
        persist_named_contexts(state, root_path)?;

        debug!(path = %self.root.display(), "Persisted state.");

        Ok(())
    }
}

fn load_conversations_and_messages_from_dir(path: &Path) -> Result<ConversationsAndMessages> {
    let conversations_path = path.join(CONVERSATIONS_DIR);
    trace!(path = %conversations_path.display(), "Loading conversations.");

    let mut conversations = TombMap::new();
    let mut messages = TombMap::new();

    for entry in fs::read_dir(&conversations_path).ok().into_iter().flatten() {
        let path = entry?.path();

        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            warn!(?path, "Skipping directory with invalid name.");
            continue;
        };

        let conversation_id = match ConversationId::from_dirname(dir_name) {
            Ok(id) => id,
            Err(error) => {
                warn!(
                    %error,
                    ?path,
                    "Failed to parse ConversationId from directory name. Skipping."
                );
                continue;
            }
        };

        let metadata_path = path.join(METADATA_FILE);
        match read_json::<Conversation>(&metadata_path) {
            Ok(conversation) => conversations.insert(conversation_id, conversation),
            Err(error) => {
                warn!(
                    %error,
                    path = metadata_path.to_string_lossy().to_string(),
                    "Failed to load conversation metadata. Skipping."
                );
                continue;
            }
        };

        let messages_path = path.join(MESSAGES_FILE);
        match read_json::<Vec<MessagePair>>(&messages_path) {
            Ok(data) => {
                messages.insert(conversation_id, data);
            }
            Err(error) => {
                warn!(%error, ?messages_path, "Failed to load messages. Skipping.");
            }
        }
    }

    Ok((conversations, messages))
}

fn persist_named_contexts(state: &State, root: &Path) -> Result<()> {
    let contexts_dir = root.join(CONTEXTS_DIR);
    trace!(path = %contexts_dir.display(), "Persisting named contexts.");

    persist_inner(
        root,
        &contexts_dir,
        &state.workspace.named_contexts,
        ContextId::to_path_buf,
    )
}

fn persist_mcp_servers(state: &State, root: &Path) -> Result<()> {
    let mcp_servers_dir = root.join(MCP_SERVERS_DIR);
    trace!(path = %mcp_servers_dir.display(), "Persisting MCP servers.");

    persist_inner(root, &mcp_servers_dir, &state.workspace.mcp_servers, |id| {
        format!("{id}.json").into()
    })
}

fn persist_conversations_and_messages(state: &State, root: &Path, local: &Path) -> Result<()> {
    let conversations_dir = root.join(CONVERSATIONS_DIR);
    let local_conversations_dir = local.join(CONVERSATIONS_DIR);

    trace!(
        global = %conversations_dir.display(),
        local = %local_conversations_dir.display(),
        "Persisting conversations."
    );

    // Append the active conversation to the list of conversations to
    // persist.
    let conversations = state.workspace.conversations.iter().chain(iter::once((
        &state.local.conversations_metadata.active_conversation_id,
        &state.workspace.active_conversation,
    )));

    for (id, conversation) in conversations {
        let dir_name = id.to_dirname(conversation.title.as_deref())?;
        let conv_dir = if conversation.private {
            local_conversations_dir.join(dir_name)
        } else {
            conversations_dir.join(dir_name)
        };

        remove_unused_conversation_dirs(
            id,
            &conv_dir,
            &conversations_dir,
            &local_conversations_dir,
        )?;

        fs::create_dir_all(&conv_dir)?;

        // Write conversation metadata
        let meta_path = conv_dir.join(METADATA_FILE);
        write_json(&meta_path, conversation)?;

        let messages = state.workspace.messages.get(id).map_or(vec![], Vec::clone);
        let messages_path = conv_dir.join(MESSAGES_FILE);
        write_json(&messages_path, &messages)?;
    }

    // Don't mark active conversation as removed.
    let mut removed_ids = state
        .workspace
        .conversations
        .removed_keys()
        .filter(|&id| id != &state.local.conversations_metadata.active_conversation_id);

    let mut deleted = Vec::new();
    for entry in conversations_dir.read_dir()?.flatten() {
        let path = entry.path();
        let name_starts_with_id = path.file_name().is_some_and(|v| {
            removed_ids.any(|d| v.to_string_lossy().starts_with(d.target_id().as_str()))
        });

        if path.is_dir() && name_starts_with_id {
            if let Ok(path) = path.strip_prefix(&conversations_dir) {
                deleted.push(path.to_path_buf());
            }
        }
    }

    remove_deleted(root, &conversations_dir, deleted.into_iter())?;

    Ok(())
}

fn remove_unused_conversation_dirs(
    id: &ConversationId,
    conversation_dir: &Path,
    workspace_conversations_dir: &Path,
    local_conversations_dir: &Path,
) -> Result<()> {
    // Gather all possible conversation directory names
    let mut dirs = vec![];
    for conversations_dir in &[workspace_conversations_dir, local_conversations_dir] {
        let pat = id.to_dirname(None)?;
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

fn persist_models(state: &State, root: &Path) -> Result<()> {
    let models_dir = root.join(MODELS_DIR);
    trace!(path = %models_dir.display(), "Persisting models.");

    persist_inner(
        root,
        &models_dir,
        &state.workspace.models,
        ModelId::to_path_buf,
    )
}

fn persist_conversations_metadata(state: &State, root: &Path) -> Result<()> {
    let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
    trace!(path = %metadata_path.display(), "Persisting local conversations metadata.");

    write_json(&metadata_path, &state.local.conversations_metadata)?;

    Ok(())
}

fn persist_personas(state: &State, root: &Path) -> Result<()> {
    let personas_dir = root.join(PERSONAS_DIR);
    trace!(path = %personas_dir.display(), "Persisting personas.");

    persist_inner(
        root,
        &personas_dir,
        &state.workspace.personas,
        PersonaId::to_path_buf,
    )
}

fn persist_inner<'a, K, V>(
    root: &Path,
    source: &Path,
    data: &'a TombMap<K, V>,
    to_path: impl Fn(&K) -> PathBuf,
) -> Result<()>
where
    K: Eq + std::hash::Hash + 'a,
    V: Serialize + 'a,
{
    fs::create_dir_all(source)?;

    let deleted = data.removed_keys().map(&to_path);
    remove_deleted(root, source, deleted)?;

    for (id, value) in data {
        let dest = source.join(to_path(id));

        // Only write if the file doesn't exist or the value has changed.
        if !dest.exists() || data.is_modified(id) {
            write_json(&dest, value)?;
        }
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

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    serde_json::from_reader(reader).map_err(Into::into)
}

pub(crate) fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let file = fs::File::create(path)?;
    let mut buf = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut buf, value)?;
    buf.write_all(b"\n")?;
    buf.flush()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        str::FromStr as _,
        time::Duration,
    };

    use jp_conversation::{model::ProviderId, Context, ConversationId};
    use jp_mcp::transport::{self, Transport};
    use tempfile::tempdir;
    use time::UtcDateTime;

    use super::*;
    use crate::state::WorkspaceState;

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
    fn test_load_local_conversations_metadata_reads_existing() {
        let original_dir = tempdir().unwrap();
        let local_dir = tempdir().unwrap();
        let meta_path = local_dir.path().join(METADATA_FILE);
        let existing_id = ConversationId::default();
        let existing_meta = ConversationsMetadata::new(existing_id);
        write_json(&meta_path, &existing_meta).unwrap();

        let storage = Storage::new(original_dir.path())
            .unwrap()
            .with_local(local_dir.path())
            .unwrap();
        let loaded_meta = storage.load_conversations_metadata().unwrap();
        assert_eq!(loaded_meta, existing_meta);
    }

    #[test]
    fn test_load_local_conversations_metadata_creates_default_when_missing() {
        let storage_dir = tempdir().unwrap();
        let local_dir = tempdir().unwrap();

        let storage = Storage::new(storage_dir.path())
            .unwrap()
            .with_local(local_dir.path())
            .unwrap();
        let loaded_meta = storage.load_conversations_metadata().unwrap();
        let default_meta = ConversationsMetadata::default();

        assert_eq!(
            loaded_meta.active_conversation_id,
            default_meta.active_conversation_id
        );
    }

    #[test]
    fn test_persist_atomic_replace_init_case() {
        let base_dir = tempdir().unwrap();
        let original_path = base_dir.path().join("persist_init_orig");

        // original_path does not exist initially
        let mut storage = Storage::new(&original_path).unwrap();
        let state = State::default();
        let persist_result = storage.persist(&state);
        assert!(
            persist_result.is_ok(),
            "Persist failed: {:?}",
            persist_result.err()
        );

        let active_conversation_id = state.local.conversations_metadata.active_conversation_id;

        assert!(original_path.exists() && original_path.is_dir());
        let final_meta_path = original_path
            .join(CONVERSATIONS_DIR)
            .join(active_conversation_id.to_dirname(None).unwrap())
            .join(METADATA_FILE);
        assert!(final_meta_path.exists());
        let final_meta: Conversation = read_json(&final_meta_path).unwrap();
        assert_eq!(final_meta, state.workspace.active_conversation);
    }

    #[test]
    fn test_load_personas_reads_existing() {
        let original_dir = tempdir().unwrap();
        let personas_orig_path = original_dir.path().join(PERSONAS_DIR);
        fs::create_dir(&personas_orig_path).unwrap();

        let persona1 = Persona {
            name: "Persona One".to_string(),
            ..Default::default()
        };
        let persona2 = Persona {
            name: "Persona Two".to_string(),
            ..Default::default()
        };
        let id1 = PersonaId::try_from("p1").unwrap();
        let id2 = PersonaId::try_from("p2").unwrap();

        write_json(&personas_orig_path.join(id1.to_path_buf()), &persona1).unwrap();
        write_json(&personas_orig_path.join(id2.to_path_buf()), &persona2).unwrap();
        fs::write(personas_orig_path.join("not-a-persona.txt"), "ignore me").unwrap(); // Non-json file

        let storage = Storage::new(original_dir.path()).unwrap();
        let loaded_personas = storage.load_personas().unwrap();

        assert_eq!(loaded_personas.len(), 3); // p1 + p2 + default
        assert_eq!(loaded_personas.get(&id1).unwrap().name, "Persona One");
        assert_eq!(loaded_personas.get(&id2).unwrap().name, "Persona Two");
    }

    #[test]
    fn test_load_personas_creates_default_if_dir_missing() {
        let original_dir = tempdir().unwrap(); // Dir exists, but PERSONAS_DIR doesn't
        let storage = Storage::new(original_dir.path()).unwrap();
        let loaded_personas = storage.load_personas().unwrap();

        assert_eq!(loaded_personas.len(), 1);
        assert!(loaded_personas.contains_key(&PersonaId::try_from("default").unwrap()));
        assert_eq!(
            loaded_personas
                .get(&PersonaId::try_from("default").unwrap())
                .unwrap()
                .name,
            "Default"
        );
    }

    #[test]
    fn test_load_personas_creates_default_if_dir_empty() {
        let original_dir = tempdir().unwrap();
        fs::create_dir(original_dir.path().join(PERSONAS_DIR)).unwrap(); // Empty dir

        let storage = Storage::new(original_dir.path()).unwrap();
        let loaded_personas = storage.load_personas().unwrap();

        assert_eq!(loaded_personas.len(), 1);
        assert!(loaded_personas.contains_key(&PersonaId::try_from("default").unwrap()));
    }

    #[test]
    fn test_load_personas_includes_default_even_if_others_exist() {
        let original_dir = tempdir().unwrap();
        let personas_orig_path = original_dir.path().join(PERSONAS_DIR);
        fs::create_dir(&personas_orig_path).unwrap();

        let persona1 = Persona {
            name: "Persona One".to_string(),
            ..Default::default()
        };
        let id1 = PersonaId::try_from("p1").unwrap();
        write_json(&personas_orig_path.join(id1.to_path_buf()), &persona1).unwrap();

        let storage = Storage::new(original_dir.path()).unwrap();
        let loaded_personas = storage.load_personas().unwrap();

        // Should load p1 AND add default if it wasn't present
        assert_eq!(loaded_personas.len(), 2);
        assert!(loaded_personas.contains_key(&id1));
        assert!(loaded_personas.contains_key(&PersonaId::try_from("default").unwrap()));
    }

    #[test]
    fn test_load_personas_handles_malformed_json() {
        let original_dir = tempdir().unwrap();
        let personas_orig_path = original_dir.path().join(PERSONAS_DIR);
        fs::create_dir(&personas_orig_path).unwrap();

        let id_good = PersonaId::try_from("good").unwrap();
        let id_bad = PersonaId::try_from("bad").unwrap();
        let persona_good = Persona {
            name: "Good".into(),
            ..Default::default()
        };

        write_json(
            &personas_orig_path.join(id_good.to_path_buf()),
            &persona_good,
        )
        .unwrap();
        fs::write(
            personas_orig_path.join(id_bad.to_path_buf()),
            "{ invalid json ",
        )
        .unwrap();

        let storage = Storage::new(original_dir.path()).unwrap();
        // Should load 'good' and 'default', warn about 'bad' (check stderr manually if needed)
        let loaded = storage.load_personas().unwrap();

        assert_eq!(loaded.len(), 2);
        assert!(loaded.contains_key(&id_good));
        assert!(loaded.contains_key(&PersonaId::try_from("default").unwrap()));
        assert!(!loaded.contains_key(&id_bad));
    }

    #[test]
    fn test_load_models_reads_existing() {
        let tmp = tempdir().unwrap();
        let models_path = tmp.path().join(MODELS_DIR);
        fs::create_dir(&models_path).unwrap();

        let model1 = Model {
            slug: "model-1".into(),
            provider: ProviderId::Openrouter,
            ..Default::default()
        };
        let model2 = Model {
            slug: "model-2".into(),
            provider: ProviderId::Openrouter,
            ..Default::default()
        };
        let id1 = ModelId::try_from("openrouter/m1").unwrap();
        let id2 = ModelId::try_from("openrouter/m2").unwrap();

        write_json(&models_path.join(id1.to_path_buf()), &model1).unwrap();
        write_json(&models_path.join(id2.to_path_buf()), &model2).unwrap();
        fs::write(models_path.join("readme.txt"), "ignore me").unwrap();

        jp_id::global::set("foo".to_owned());
        let storage = Storage::new(tmp.path()).unwrap();
        let loaded_models = storage.load_models().unwrap();

        assert_eq!(loaded_models.len(), 2);
        assert_eq!(loaded_models.get(&id1).unwrap().slug, "model-1");
        assert_eq!(loaded_models.get(&id2).unwrap().slug, "model-2");
    }

    #[test]
    fn test_load_models_has_default_if_dir_missing() {
        let original_dir = tempdir().unwrap(); // MODELS_DIR doesn't exist
        let storage = Storage::new(original_dir.path()).unwrap();
        let loaded_models = storage.load_models().unwrap();

        assert_eq!(loaded_models.len(), 1); // default
    }

    #[test]
    fn test_load_models_invalid_provider() {
        let tmp = tempdir().unwrap();
        let models_path = tmp.path().join(MODELS_DIR);
        fs::create_dir_all(models_path.join("openrouter")).unwrap();

        let model1 = Model {
            slug: "model-1".into(),
            ..Default::default()
        };
        let id1 = ModelId::try_from("openrouter/m1").unwrap();

        write_json(
            &models_path.join("invalid_provider").join(id1.to_path_buf()),
            &model1,
        )
        .unwrap();

        let storage = Storage::new(tmp.path()).unwrap();
        let loaded_models = storage.load_models().unwrap();

        assert_eq!(loaded_models.len(), 1); // default
        assert!(!loaded_models.contains_key(&id1));
    }

    #[test]
    fn test_persist_writes_models_and_deletes_stale() {
        let tmp = tempdir().unwrap();

        let path = tmp.path();
        let provider_path = path.join(MODELS_DIR);
        fs::create_dir(&provider_path).unwrap();

        let id_m1 = ModelId::try_from("openrouter/m1").unwrap();
        let id_stale = ModelId::try_from("openrouter/m_stale").unwrap();

        write_json(&provider_path.join(id_m1.to_path_buf()), &Model {
            slug: "old".into(),
            ..Default::default()
        })
        .unwrap();
        write_json(&provider_path.join(id_stale.to_path_buf()), &Model {
            slug: "stale".into(),
            ..Default::default()
        })
        .unwrap();

        let mut storage = Storage::new(path).unwrap();

        // Prepare new state: m1 (updated) and m2 (new)
        let id_m2 = ModelId::try_from("openrouter/m2").unwrap();
        let mut new_models = TombMap::new();
        new_models.insert(id_stale.clone(), Model {
            slug: "stale".into(),
            ..Default::default()
        });
        new_models.insert(id_m1.clone(), Model {
            slug: "old".into(),
            ..Default::default()
        });
        new_models.insert(id_m2.clone(), Model {
            slug: "new".into(),
            ..Default::default()
        });
        let mut new_state = State {
            workspace: WorkspaceState {
                models: new_models,
                ..Default::default()
            },
            ..Default::default()
        };

        new_state.workspace.models.remove(&id_stale);
        new_state.workspace.models.get_mut(&id_m1).unwrap().slug = "updated".into();

        storage.persist(&new_state).unwrap();

        assert!(provider_path.join(id_m1.to_path_buf()).exists());
        assert!(provider_path.join(id_m2.to_path_buf()).exists());
        assert!(!provider_path.join(id_stale.to_path_buf()).exists());
        let m1_final: Model = read_json(&provider_path.join(id_m1.to_path_buf())).unwrap();
        assert_eq!(m1_final.slug, "updated");
    }

    #[test]
    fn test_conversation_dir_name_generation() {
        let id = ConversationId::from_str("jp-c17457886043-otvo8").unwrap();
        assert_eq!(id.to_dirname(None).unwrap(), "17457886043");
        assert_eq!(
            id.to_dirname(Some("Simple Title")).unwrap(),
            "17457886043-simple-title"
        );
        assert_eq!(
            id.to_dirname(Some(" Title with spaces & chars!")).unwrap(),
            "17457886043-title-with-spaces---chars" // Sanitized
        );
        assert_eq!(
            id.to_dirname(Some(
                "A very long title that definitely exceeds the sixty character limit for testing \
                 purposes"
            ))
            .unwrap(),
            "17457886043-a-very-long-title-that-definitely-exceeds-the-sixty" // Truncated
        );
        assert_eq!(
            id.to_dirname(Some("")).unwrap(), // Empty title
            "17457886043"
        );
    }

    #[test]
    fn test_load_conversations_and_messages() {
        let original_dir = tempdir().unwrap();
        let storage = Storage::new(original_dir.path()).unwrap(); // Storage uses temp copy

        // Setup: Create conversation directories in the *storage's* temp dir
        let conv_dir_path = storage.root.join(CONVERSATIONS_DIR);
        fs::create_dir(&conv_dir_path).unwrap();

        let now = UtcDateTime::now();
        let id1 = ConversationId::try_from(now - Duration::from_secs(24 * 60 * 60)).unwrap();
        let id2 = ConversationId::try_from(now).unwrap();

        let context1 = Context::new(PersonaId::try_from("default").unwrap());
        let context2 = Context::new(PersonaId::try_from("other").unwrap());

        let conv1_dir = conv_dir_path.join(id1.to_dirname(Some("Conv 1")).unwrap());
        fs::create_dir(&conv1_dir).unwrap();
        let conv1 = Conversation {
            last_activated_at: UtcDateTime::now(),
            title: Some("Conv 1".into()),
            context: context1.clone(),
            ..Default::default()
        };
        write_json(&conv1_dir.join(METADATA_FILE), &conv1).unwrap();
        let messages1 = vec![MessagePair::new("Q1".into(), "R1".into()).with_context(context1)];
        write_json(&conv1_dir.join(MESSAGES_FILE), &messages1).unwrap();

        let conv2_dir = conv_dir_path.join(id2.to_dirname(None).unwrap());
        fs::create_dir(&conv2_dir).unwrap();
        let conv2 = Conversation {
            last_activated_at: UtcDateTime::now(),
            title: None,
            context: context2.clone(),
            ..Default::default()
        };
        write_json(&conv2_dir.join(METADATA_FILE), &conv2).unwrap();
        // No messages file for conv2

        // Action: Load conversations and messages
        let (loaded_convs, loaded_msgs) = storage.load_conversations_and_messages().unwrap();

        // Assertions
        assert_eq!(loaded_convs.len(), 2);
        assert_eq!(loaded_msgs.len(), 1); // Only conv1 had messages

        assert_eq!(loaded_convs.get(&id1).unwrap().title, Some("Conv 1".into()));
        assert_eq!(loaded_convs.get(&id2).unwrap().title, None);

        assert_eq!(loaded_msgs.get(&id1).unwrap().len(), 1);
        assert_eq!(loaded_msgs.get(&id1).unwrap()[0].message, "Q1".into());
        assert!(!loaded_msgs.contains_key(&id2)); // No messages for conv2
    }

    #[test]
    fn test_load_mcp_servers() {
        let original_dir = tempdir().unwrap();
        let storage = Storage::new(original_dir.path()).unwrap();
        let mcp_path = storage.root.join(MCP_SERVERS_DIR);
        fs::create_dir(&mcp_path).unwrap();

        let id1 = McpServerId::new("server1");
        let server1 = McpServer {
            id: id1.clone(),
            transport: Transport::Stdio(transport::Stdio {
                command: "/bin/echo".into(),
                args: vec!["hello".into()],
                environment_variables: vec![],
            }),
        };
        write_json(&mcp_path.join(format!("{id1}.json")), &server1).unwrap();

        let loaded = storage.load_mcp_servers().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key(&id1));
    }

    #[test]
    fn test_persist_mcp_servers() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let mut storage = Storage::new(root).unwrap();

        let id = McpServerId::new("foo");
        let server = McpServer {
            id: id.clone(),
            transport: Transport::Stdio(transport::Stdio {
                command: "/usr/bin/tool".into(),
                args: vec![],
                environment_variables: vec![],
            }),
        };
        let state = State {
            workspace: WorkspaceState {
                mcp_servers: TombMap::from([(id.clone(), server.clone())]),
                ..Default::default()
            },
            ..Default::default()
        };
        storage.persist(&state).unwrap();

        let servers_path = root.join(MCP_SERVERS_DIR);
        assert!(servers_path.is_dir());
        assert!(servers_path.join(format!("{id}.json")).is_file());

        let storage = Storage::new(root).unwrap();
        let servers = storage.load_mcp_servers().unwrap();
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key(&id));
    }

    #[test]
    fn test_load_named_contexts() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let contexts_path = root.join(CONTEXTS_DIR);
        fs::create_dir(&contexts_path).unwrap();

        let id1 = ContextId::try_from("foo").unwrap();
        let ctx1 = Context::new(PersonaId::try_from("p1").unwrap());

        let id2 = ContextId::try_from("bar").unwrap();
        let ctx2 = Context::new(PersonaId::try_from("p2").unwrap());

        write_json(&contexts_path.join(id1.to_path_buf()), &ctx1).unwrap();
        write_json(&contexts_path.join(id2.to_path_buf()), &ctx2).unwrap();
        fs::write(contexts_path.join("ignore_me.txt"), "data").unwrap();

        let storage = Storage::new(root).unwrap();
        let loaded = storage.load_named_contexts().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(&id1), Some(&ctx1));
        assert_eq!(loaded.get(&id2), Some(&ctx2));
    }

    #[test]
    fn test_persist_named_contexts() {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let mut storage = Storage::new(root).unwrap();

        let id = ContextId::try_from("ctx-gamma").unwrap();
        let ctx = Context::new(PersonaId::try_from("default").unwrap());
        let state = State {
            workspace: WorkspaceState {
                named_contexts: TombMap::from([(id.clone(), ctx.clone())]),
                ..Default::default()
            },
            ..Default::default()
        };
        storage.persist(&state).unwrap();

        let contexts_path = root.join(CONTEXTS_DIR);
        assert!(contexts_path.is_dir());
        assert!(contexts_path.join(id.to_path_buf()).is_file());

        let storage = Storage::new(root).unwrap();
        let ctxs = storage.load_named_contexts().unwrap();
        assert_eq!(ctxs.len(), 1);
        assert_eq!(ctxs.get(&id), Some(&ctx));
    }
}
