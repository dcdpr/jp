//! Handles the physical storage aspects, including temporary copying and persistence.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufReader, BufWriter},
    iter,
    path::{Path, PathBuf},
    str::FromStr as _,
};

use jp_conversation::{
    model::ProviderId, Context, ContextId, Conversation, ConversationId, MessagePair, Model,
    ModelId, Persona, PersonaId,
};
use jp_mcp::config::{McpServer, McpServerId};
use serde::{de::DeserializeOwned, Serialize};
use tempfile::TempDir;
use tracing::{debug, trace, warn};

use crate::{
    error::{Error, Result},
    state::ConversationsMetadata,
    State,
};

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

    /// The temporary directory we are operating on.
    tmpdir: TempDir,
}

impl Storage {
    /// Creates a new Storage instance by creating a temporary directory and
    /// copying the contents of `root` into it.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Result<Self> {
        // Create temporary directory.
        let tmpdir = tempfile::Builder::new().prefix("jp_storage_").tempdir()?;
        trace!(tmp = %tmpdir.path().display(), "Created temporary storage directory.");

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

        // Copy root storage directory to temporary directory.
        let dst = tmpdir.path().to_path_buf();
        trace!(path = %root.display(), "Copying storage directory.");
        copy_dir_recursive(&root, &dst)?;

        Ok(Self {
            root,
            local: None,
            tmpdir,
        })
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
    pub(crate) fn load_personas(&self) -> Result<HashMap<PersonaId, Persona>> {
        let personas_path = self.tmpdir.path().join(PERSONAS_DIR);
        trace!(path = %personas_path.display(), "Loading personas.");

        let mut personas = HashMap::new();

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
    pub(crate) fn load_models(&self) -> Result<HashMap<ModelId, Model>> {
        let models_path = self.tmpdir.path().join(MODELS_DIR);
        trace!(path = %models_path.display(), "Loading models.");

        let mut models = HashMap::new();

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
                models.insert(ModelId::from_filename(id)?, model);
            }
        }

        if models.is_empty() {
            models.insert(ModelId::default(), Model::default());
        }

        Ok(models)
    }

    /// Loads all MCP Servers from the (copied) storage.
    pub(crate) fn load_mcp_servers(&self) -> Result<HashMap<McpServerId, McpServer>> {
        let mcp_path = self.tmpdir.path().join(MCP_SERVERS_DIR);
        trace!(path = %mcp_path.display(), "Loading MCP servers.");

        let mut servers = HashMap::new();

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
    pub(crate) fn load_named_contexts(&self) -> Result<HashMap<ContextId, Context>> {
        let contexts_path = self.tmpdir.path().join(CONTEXTS_DIR);
        trace!(path = %contexts_path.display(), "Loading named contexts.");

        let mut contexts = HashMap::new();

        for entry in fs::read_dir(&contexts_path).ok().into_iter().flatten() {
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
            let Ok(id) = id_str.parse::<ContextId>() else {
                warn!(?path, "Invalid ContextId in filename. Skipping.");
                continue;
            };
            let context = match read_json::<Context>(&path) {
                Ok(context) => context,
                Err(error) => {
                    warn!(?path, ?error, "Failed to read NamedContext file. Skipping.");
                    continue;
                }
            };

            contexts.insert(id, context);
        }

        Ok(contexts)
    }

    /// Loads all conversations and their associated messages.
    #[allow(clippy::type_complexity)]
    pub(crate) fn load_conversations_and_messages(
        &self,
    ) -> Result<(
        HashMap<ConversationId, Conversation>,
        HashMap<ConversationId, Vec<MessagePair>>,
    )> {
        let conversations_path = self.tmpdir.path().join(CONVERSATIONS_DIR);
        trace!(path = %conversations_path.display(), "Loading conversations.");

        let mut conversations = HashMap::new();
        let mut messages = HashMap::new();

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

    /// Persists the entire storage state to disk atomically.
    pub(crate) fn persist(&mut self, state: &State) -> Result<()> {
        trace!("Persisting state.");

        // Some data is stored locally, if configured, otherwise it is stored
        // in the workspace storage.
        let temp_path = self.tmpdir.path();
        let local_or_temp_path = self.local.as_deref().unwrap_or(temp_path);

        // Step 1: Write state to local or temp dir
        persist_conversations_metadata(state, local_or_temp_path)?;
        persist_personas(state, temp_path)?;
        persist_models(state, temp_path)?;
        persist_conversations_and_messages(state, temp_path)?;
        persist_mcp_servers(state, temp_path)?;
        persist_named_contexts(state, temp_path)?;

        // Step 2: Atomic replace
        let original_path = &self.root;
        let backup_path = original_path.with_extension("bak");

        // Cleanup old backup
        if backup_path.is_dir() {
            fs::remove_dir_all(&backup_path)?;
        } else if backup_path.try_exists()? {
            fs::remove_file(&backup_path)?;
        }

        // Rename original to backup
        let original_existed = original_path.exists();
        if original_existed {
            fs::rename(original_path, &backup_path)?;
        }

        // Ensure storage parent dir exists
        if let Some(parent) = original_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Rename temp to original
        if let Err(rename_error) = fs::rename(temp_path, original_path) {
            // Attempt restore
            if original_existed {
                fs::rename(&backup_path, original_path)?;
            }

            return Err(Error::AtomicReplaceFailed {
                src: temp_path.to_path_buf(),
                dst: original_path.clone(),
                error: rename_error,
            });
        }

        // Step 3: Cleanup backup
        if original_existed && backup_path.is_dir() {
            fs::remove_dir_all(&backup_path)?;
        }

        debug!(path = %self.root.display(), "Persisted state.");

        Ok(())
    }
}

fn persist_named_contexts(state: &State, source: &Path) -> Result<()> {
    let contexts_source = source.join(CONTEXTS_DIR);
    trace!(path = %contexts_source.display(), "Persisting named contexts.");

    persist_inner(source, &contexts_source, |written| {
        for (id, context) in &state.workspace.named_contexts {
            let context_file_path = contexts_source.join(format!("{id}.json"));
            write_json(&context_file_path, context)?;
            written.insert(context_file_path);
        }

        Ok(())
    })
}

fn persist_mcp_servers(state: &State, source: &Path) -> Result<()> {
    let mcp_source = source.join(MCP_SERVERS_DIR);
    trace!(path = %mcp_source.display(), "Persisting MCP servers.");

    persist_inner(source, &mcp_source, |written| {
        for (id, server) in &state.workspace.mcp_servers {
            let server_file_path = mcp_source.join(format!("{id}.json"));
            write_json(&server_file_path, server)?;
            written.insert(server_file_path);
        }

        Ok(())
    })
}

fn persist_conversations_and_messages(state: &State, source: &Path) -> Result<()> {
    let conversations_source = source.join(CONVERSATIONS_DIR);
    trace!(path = %conversations_source.display(), "Persisting conversations.");

    persist_inner(source, &conversations_source, |written| {
        // Append the active conversation to the list of conversations to
        // persist.
        let conversations = state.workspace.conversations.iter().chain(iter::once((
            &state.local.conversations_metadata.active_conversation_id,
            &state.workspace.active_conversation,
        )));

        for (id, conversation) in conversations {
            // Determine directory name based on current title
            let dir_name = id.to_dirname(conversation.title.as_deref())?;
            let conv_dir = conversations_source.join(dir_name);
            fs::create_dir_all(&conv_dir)?;

            // Write conversation metadata
            let meta_path = conv_dir.join(METADATA_FILE);
            write_json(&meta_path, conversation)?;

            let messages = state.workspace.messages.get(id).map_or(vec![], Vec::clone);
            let messages_path = conv_dir.join(MESSAGES_FILE);
            write_json(&messages_path, &messages)?;

            written.insert(meta_path);
            written.insert(messages_path);
        }

        Ok(())
    })
}

fn persist_models(state: &State, source: &Path) -> Result<()> {
    let models_source = source.join(MODELS_DIR);
    trace!(path = %models_source.display(), "Persisting models.");

    persist_inner(source, &models_source, |written| {
        for (id, model) in &state.workspace.models {
            let model_file_path = models_source
                .join(model.provider.to_string())
                .join(id.to_filename());
            write_json(&model_file_path, model)?;
            written.insert(model_file_path);
        }

        Ok(())
    })
}

fn persist_conversations_metadata(state: &State, source: &Path) -> Result<()> {
    let metadata_path = source.join(CONVERSATIONS_DIR).join(METADATA_FILE);
    trace!(path = %metadata_path.display(), "Persisting local conversations metadata.");

    write_json(&metadata_path, &state.local.conversations_metadata)?;

    Ok(())
}

fn persist_personas(state: &State, source: &Path) -> Result<()> {
    let personas_source = source.join(PERSONAS_DIR);
    trace!(path = %personas_source.display(), "Persisting personas.");

    persist_inner(source, &personas_source, |written| {
        for (id, persona) in &state.workspace.personas {
            let persona_file_path = personas_source.join(id.to_filename());

            write_json(&persona_file_path, persona)?;
            written.insert(persona_file_path);
        }

        Ok(())
    })
}

fn persist_inner(
    root: &Path,
    source: &Path,
    write: impl FnOnce(&mut HashSet<PathBuf>) -> Result<()>,
) -> Result<()> {
    fs::create_dir_all(source)?;

    let existing_files = find_json_files_in_dir(source)?;
    let mut written_files = HashSet::new();
    write(&mut written_files)?;

    for path_to_delete in existing_files.difference(&written_files) {
        fs::remove_file(path_to_delete)?;

        // Remove empty parent directories, until we reach the root.
        let mut path = path_to_delete.as_path();
        while let Some(parent) = path.parent() {
            if parent.as_os_str() == "" || parent == root || !parent.is_dir() {
                break;
            }
            if fs::read_dir(parent)?.count() != 0 {
                break;
            }

            fs::remove_dir(parent)?;
            path = parent;
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
    serde_json::to_writer_pretty(BufWriter::new(file), value).map_err(Into::into)
}

/// Recursively copies the contents of a directory.
///
/// Creates `dst` if it doesn't exist.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::result::Result<(), std::io::Error> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let Some(file_name) = src_path.file_name() else {
            continue;
        };

        let dst_path = dst.join(file_name);
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

fn find_json_files_in_dir(dir: &Path) -> Result<HashSet<PathBuf>> {
    let mut files = HashSet::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            files.extend(find_json_files_in_dir(&path)?);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            files.insert(path);
        }
    }
    Ok(files)
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
    fn test_storage_new_creates_temp_dir() {
        let source_dir = tempdir().unwrap();
        let storage = Storage::new(source_dir.path()).unwrap();

        assert!(storage.tmpdir.path().is_dir());
        assert!(storage.tmpdir.path() != source_dir.path());
        assert_eq!(storage.root, source_dir.path());
    }

    #[test]
    fn test_storage_new_copies_content_flat() {
        let source_dir = tempdir().unwrap();
        let source_file_path = source_dir.path().join("test.txt");
        let file_content = "Hello, world!";
        fs::write(&source_file_path, file_content).unwrap();

        let storage = Storage::new(source_dir.path()).unwrap();
        let dest_file_path = storage.tmpdir.path().join("test.txt");

        assert!(dest_file_path.is_file());
        assert_eq!(fs::read_to_string(&dest_file_path).unwrap(), file_content);
    }

    #[test]
    fn test_storage_new_copies_content_recursive() {
        let source_dir = tempdir().unwrap();
        let sub_dir_path = source_dir.path().join("subdir");
        fs::create_dir(&sub_dir_path).unwrap();
        let source_file_path = sub_dir_path.join("nested.txt");
        let file_content = "Nested content";
        fs::write(&source_file_path, file_content).unwrap();

        let storage = Storage::new(source_dir.path()).unwrap();
        let dest_sub_dir = storage.tmpdir.path().join("subdir");
        let dest_file_path = dest_sub_dir.join("nested.txt");

        assert!(dest_sub_dir.is_dir());
        assert!(dest_file_path.is_file());
        let read_content = fs::read_to_string(&dest_file_path).unwrap();
        assert_eq!(read_content, file_content);
    }

    #[test]
    fn test_storage_handles_missing_src() {
        let missing_path = PathBuf::from("./non_existent_jp_workspace_source_dir_abc123");
        assert!(!missing_path.exists());

        let storage = Storage::new(&missing_path).expect("must succeed");
        assert!(storage.tmpdir.path().is_dir());
        assert_eq!(fs::read_dir(storage.tmpdir.path()).unwrap().count(), 0);
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
    fn test_storage_temp_dir_cleanup_implicit() {
        let source_dir = tempdir().unwrap();
        let storage = Storage::new(source_dir.path()).unwrap();
        let temp_path = storage.tmpdir.path().to_path_buf();
        assert!(temp_path.exists());

        drop(storage); // Explicitly drop to trigger cleanup
        assert!(
            !temp_path.exists(),
            "Temporary directory should be cleaned up"
        );
    }

    #[test]
    fn copy_dir_recursive_handles_empty_dir() {
        let src = tempdir().unwrap();
        let dst = tempdir().unwrap();
        let dst_path = dst.path().join("target");

        let result = copy_dir_recursive(src.path(), &dst_path);
        assert!(result.is_ok());
        assert!(dst_path.is_dir());
        assert_eq!(fs::read_dir(&dst_path).unwrap().count(), 0);
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

        write_json(&personas_orig_path.join(id1.to_filename()), &persona1).unwrap();
        write_json(&personas_orig_path.join(id2.to_filename()), &persona2).unwrap();
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
        write_json(&personas_orig_path.join(id1.to_filename()), &persona1).unwrap();

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
            &personas_orig_path.join(id_good.to_filename()),
            &persona_good,
        )
        .unwrap();
        fs::write(
            personas_orig_path.join(id_bad.to_filename()),
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
    fn test_persist_writes_personas_and_deletes_stale() {
        let original_dir = tempdir().unwrap();
        let original_path = original_dir.path();

        // Setup initial state with p1 and p_stale
        let personas_orig_path = original_path.join(PERSONAS_DIR);
        fs::create_dir(&personas_orig_path).unwrap();
        let id_p1 = PersonaId::try_from("p1").unwrap();
        let id_stale = PersonaId::try_from("p_stale").unwrap();
        write_json(
            &personas_orig_path.join(id_p1.to_filename()),
            &Persona::default(),
        )
        .unwrap();
        write_json(
            &personas_orig_path.join(id_stale.to_filename()),
            &Persona::default(),
        )
        .unwrap();

        // Load (copies p1 and p_stale to temp)
        let mut storage = Storage::new(original_path).unwrap();

        // Prepare new state: p1 (updated) and p2 (new), default (always included)
        // p_stale is omitted from the new state.
        let id_p2 = PersonaId::try_from("p2").unwrap();
        let mut new_personas = HashMap::new();
        new_personas.insert(id_p1.clone(), Persona {
            name: "P1 Updated".into(),
            ..Default::default()
        });
        new_personas.insert(id_p2.clone(), Persona {
            name: "P2 New".into(),
            ..Default::default()
        });
        new_personas.insert(PersonaId::try_from("default").unwrap(), Persona::default()); // Ensure default persists

        let new_state = State {
            workspace: WorkspaceState {
                personas: new_personas,
                ..Default::default()
            },
            ..Default::default()
        };

        // Persist
        storage.persist(&new_state).unwrap();

        // Verify final state in original directory
        let final_personas_path = original_path.join(PERSONAS_DIR);
        assert!(final_personas_path.exists());

        let p1_final_path = final_personas_path.join(id_p1.to_filename());
        let p2_final_path = final_personas_path.join(id_p2.to_filename());
        let default_final_path =
            final_personas_path.join(PersonaId::try_from("default").unwrap().to_filename());
        let stale_final_path = final_personas_path.join(id_stale.to_filename());

        assert!(p1_final_path.exists());
        assert!(p2_final_path.exists());
        assert!(default_final_path.exists()); // Check default exists
        assert!(
            !stale_final_path.exists(),
            "Stale persona file should be deleted"
        );

        // Verify content of updated persona
        let p1_final: Persona = read_json(&p1_final_path).unwrap();
        assert_eq!(p1_final.name, "P1 Updated");
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
        let id1 = ModelId::try_from("m1").unwrap();
        let id2 = ModelId::try_from("m2").unwrap();

        write_json(
            &models_path
                .join(model1.provider.to_string())
                .join(id1.to_filename()),
            &model1,
        )
        .unwrap();
        write_json(
            &models_path
                .join(model2.provider.to_string())
                .join(id2.to_filename()),
            &model2,
        )
        .unwrap();
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
        fs::create_dir(&models_path).unwrap();

        let model1 = Model {
            slug: "model-1".into(),
            ..Default::default()
        };
        let id1 = ModelId::try_from("m1").unwrap();

        write_json(
            &models_path.join("invalid_provider").join(id1.to_filename()),
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
        let provider_path = path.join(MODELS_DIR).join("openrouter");
        fs::create_dir_all(&provider_path).unwrap();

        let id_m1 = ModelId::try_from("m1").unwrap();
        let id_stale = ModelId::try_from("m_stale").unwrap();

        write_json(&provider_path.join(id_m1.to_filename()), &Model {
            slug: "old".into(),
            ..Default::default()
        })
        .unwrap();
        write_json(&provider_path.join(id_stale.to_filename()), &Model {
            slug: "stale".into(),
            ..Default::default()
        })
        .unwrap();

        let mut storage = Storage::new(path).unwrap();

        // Prepare new state: m1 (updated) and m2 (new)
        let id_m2 = ModelId::try_from("m2").unwrap();
        let mut new_models = HashMap::new();
        new_models.insert(id_m1.clone(), Model {
            slug: "updated".into(),
            ..Default::default()
        });
        new_models.insert(id_m2.clone(), Model {
            slug: "new".into(),
            ..Default::default()
        });
        let new_state = State {
            workspace: WorkspaceState {
                models: new_models,
                ..Default::default()
            },
            ..Default::default()
        };

        storage.persist(&new_state).unwrap();

        assert!(provider_path.join(id_m1.to_filename()).exists());
        assert!(provider_path.join(id_m2.to_filename()).exists());
        assert!(
            !provider_path.join(id_stale.to_filename()).exists(),
            "Stale model deleted"
        );
        let m1_final: Model = read_json(&provider_path.join(id_m1.to_filename())).unwrap();
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
        let conv_dir_path = storage.tmpdir.path().join(CONVERSATIONS_DIR);
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
        let mcp_path = storage.tmpdir.path().join(MCP_SERVERS_DIR);
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
                mcp_servers: HashMap::from([(id.clone(), server.clone())]),
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

        let id1 = ContextId::new("foo");
        let ctx1 = Context::new(PersonaId::try_from("p1").unwrap());

        let id2 = ContextId::new("bar");
        let ctx2 = Context::new(PersonaId::try_from("p2").unwrap());

        write_json(&contexts_path.join(format!("{id1}.json")), &ctx1).unwrap();
        write_json(&contexts_path.join(format!("{id2}.json")), &ctx2).unwrap();
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

        let id = ContextId::new("ctx-gamma");
        let ctx = Context::new(PersonaId::try_from("default").unwrap());
        let state = State {
            workspace: WorkspaceState {
                named_contexts: HashMap::from([(id.clone(), ctx.clone())]),
                ..Default::default()
            },
            ..Default::default()
        };
        storage.persist(&state).unwrap();

        let contexts_path = root.join(CONTEXTS_DIR);
        assert!(contexts_path.is_dir());
        assert!(contexts_path.join(format!("{id}.json")).is_file());

        let storage = Storage::new(root).unwrap();
        let ctxs = storage.load_named_contexts().unwrap();
        assert_eq!(ctxs.len(), 1);
        assert_eq!(ctxs.get(&id), Some(&ctx));
    }
}
