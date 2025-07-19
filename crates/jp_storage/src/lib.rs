pub mod error;
pub mod value;

use std::{
    ffi::OsStr,
    fs, iter,
    path::{Path, PathBuf},
};

pub use error::Error;
use jp_conversation::{Conversation, ConversationId, ConversationsMetadata, MessagePair};
use jp_id::Id as _;
use jp_mcp::{
    config::{McpServer, McpServerId},
    tool::{McpTool, McpToolId, McpToolsMetadata},
};
use jp_tombmap::TombMap;
use serde::Serialize;
use serde_json::Value;
use tracing::{trace, warn};

use crate::{
    error::Result,
    value::{deep_merge, read_json, write_json},
};

type ConversationsAndMessages = (
    TombMap<ConversationId, Conversation>,
    TombMap<ConversationId, Vec<MessagePair>>,
);

pub const DEFAULT_STORAGE_DIR: &str = ".jp";
pub const METADATA_FILE: &str = "metadata.json";
const MESSAGES_FILE: &str = "messages.json";
pub const CONVERSATIONS_DIR: &str = "conversations";
pub const MCP_SERVERS_DIR: &str = "mcp/servers";
pub const MCP_TOOLS_DIR: &str = "mcp/tools";

#[derive(Debug)]
pub struct Storage {
    /// The path to the original storage directory.
    root: PathBuf,

    /// The path to the user storage directory.
    ///
    /// This is used (among other things) to store the active conversation id
    /// that are tied to the current user.
    ///
    /// If unset, user storage is disabled.
    user: Option<PathBuf>,
}

impl Storage {
    /// Creates a new Storage instance by creating a temporary directory and
    /// copying the contents of `root` into it.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
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

        Ok(Self { root, user: None })
    }

    pub fn with_user_storage(
        mut self,
        root: &Path,
        name: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self> {
        let name: String = name.into();
        let id: String = id.into();
        let mut path = root.join(format!("{name}-{id}"));

        // Create user storage directory, if needed.
        if root.exists()
            && let Some(existing_path) = fs::read_dir(root)?.find_map(|entry| {
                let path = entry.ok()?.path();
                path.to_string_lossy().ends_with(&id).then_some(path)
            })
        {
            if !existing_path.is_dir() {
                return Err(Error::NotDir(existing_path));
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
            trace!(path = %path.display(), "Created user storage directory.");
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
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// Returns the path to the user storage directory, if configured.
    #[must_use]
    pub fn user_storage_path(&self) -> Option<&Path> {
        self.user.as_deref()
    }

    /// Loads the conversations metadata from storage.
    ///
    /// This loads the file from user storage if configured, otherwise the
    /// workspace storage is used.
    ///
    /// If the file does not exist, return default conversations metadata.
    pub fn load_conversations_metadata(&self) -> Result<ConversationsMetadata> {
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        trace!(path = %metadata_path.display(), "Loading user conversations metadata.");

        if !metadata_path.exists() {
            return Ok(ConversationsMetadata::default());
        }

        read_json(&metadata_path)
    }

    /// Loads all MCP Servers from the (copied) storage.
    pub fn load_mcp_servers(&self) -> Result<TombMap<McpServerId, McpServer>> {
        let mcp_path = self.root.join(MCP_SERVERS_DIR);
        let user_mcp_path = self.user.as_ref().map(|p| p.join(MCP_SERVERS_DIR));
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
            let server = match read_json::<Value>(&path) {
                Ok(value) => value,
                Err(error) => {
                    warn!(?path, ?error, "Failed to read MCP server file. Skipping.");
                    continue;
                }
            };

            // Merge user server config on top of workspace server config.
            let mut server: McpServer = match user_mcp_path.as_ref().map(|p| p.join(filename)) {
                Some(p) if p.is_file() => match read_json::<Value>(&p) {
                    Err(error) => {
                        warn!(?path, ?error, "Failed to read MCP server file. Skipping.");
                        continue;
                    }
                    Ok(user) => deep_merge(server, user)?,
                },
                _ => serde_json::from_value(server)?,
            };

            let id = McpServerId::new(id_str);
            server.id = id.clone();
            servers.insert(id, server);
        }

        Ok(servers)
    }

    /// Loads all MCP tools from the storage.
    pub fn load_mcp_tools(&self) -> Result<TombMap<McpToolId, McpTool>> {
        let tools_path = self.root.join(MCP_TOOLS_DIR);
        trace!(path = %tools_path.display(), "Loading MCP tools.");

        let mut tools = TombMap::new();
        for entry in fs::read_dir(&tools_path).ok().into_iter().flatten() {
            recurse_mcp_tools_dirs(&tools_path, &entry?.path(), &mut tools)?;
        }

        Ok(tools)
    }

    /// Loads all conversations and their associated messages, including user
    /// conversations.
    pub fn load_conversations_and_messages(&self) -> Result<ConversationsAndMessages> {
        let (mut conversations, mut messages) =
            load_conversations_and_messages_from_dir(&self.root)?;

        if let Some(user) = self.user.as_ref() {
            let (mut user_conversations, user_messages) =
                load_conversations_and_messages_from_dir(user)?;

            for (_, conversation) in user_conversations.iter_mut_untracked() {
                conversation.user = true;
            }

            conversations.extend(user_conversations);
            messages.extend(user_messages);
        }

        Ok((conversations, messages))
    }

    pub fn persist_mcp_servers(&mut self, servers: &TombMap<McpServerId, McpServer>) -> Result<()> {
        let root = self.root.as_path();
        let mcp_servers_dir = root.join(MCP_SERVERS_DIR);
        trace!(path = %mcp_servers_dir.display(), "Persisting MCP servers.");

        persist_inner(root, &mcp_servers_dir, servers, |id| {
            format!("{id}.json").into()
        })
    }

    pub fn persist_conversations_and_messages(
        &mut self,
        conversations: &TombMap<ConversationId, Conversation>,
        messages: &TombMap<ConversationId, Vec<MessagePair>>,
        active_conversation_id: &ConversationId,
        active_conversation: &Conversation,
    ) -> Result<()> {
        let root = self.root.as_path();
        let user = self.user.as_deref().unwrap_or(root);

        let conversations_dir = root.join(CONVERSATIONS_DIR);
        let user_conversations_dir = user.join(CONVERSATIONS_DIR);

        trace!(
            global = %conversations_dir.display(),
            user = %user_conversations_dir.display(),
            "Persisting conversations."
        );

        // Append the active conversation to the list of conversations to
        // persist.
        let all_conversations = conversations
            .iter()
            .chain(iter::once((active_conversation_id, active_conversation)));

        for (id, conversation) in all_conversations {
            let dir_name = id.to_dirname(conversation.title.as_deref())?;
            let conv_dir = if conversation.user {
                user_conversations_dir.join(dir_name)
            } else {
                conversations_dir.join(dir_name)
            };

            remove_unused_conversation_dirs(
                id,
                &conv_dir,
                &conversations_dir,
                &user_conversations_dir,
            )?;

            fs::create_dir_all(&conv_dir)?;

            // Write conversation metadata
            let meta_path = conv_dir.join(METADATA_FILE);
            write_json(&meta_path, conversation)?;

            let messages = messages.get(id).map_or(vec![], Vec::clone);
            let messages_path = conv_dir.join(MESSAGES_FILE);
            write_json(&messages_path, &messages)?;
        }

        // Don't mark active conversation as removed.
        let removed_ids = conversations
            .removed_keys()
            .filter(|&id| id != active_conversation_id)
            .collect::<Vec<_>>();

        for dir in [&conversations_dir, &user_conversations_dir] {
            let mut deleted = Vec::new();
            for entry in dir.read_dir()?.flatten() {
                let path = entry.path();
                let dir_matches_id = path.file_name().is_some_and(|v| {
                    removed_ids.iter().any(|d| {
                        let file_name = v.to_string_lossy();
                        let removed_id = d.target_id();

                        file_name == *removed_id || file_name.starts_with(&format!("{removed_id}-"))
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
        let root = self.user.as_deref().unwrap_or(self.root.as_path());
        let metadata_path = root.join(CONVERSATIONS_DIR).join(METADATA_FILE);
        trace!(path = %metadata_path.display(), "Persisting user conversations metadata.");

        write_json(&metadata_path, metadata)?;

        Ok(())
    }
}

fn recurse_mcp_tools_dirs(
    root: &Path,
    path: &Path,
    tools: &mut TombMap<McpToolId, McpTool>,
) -> Result<()> {
    let metadata = read_json::<McpToolsMetadata>(&root.join(METADATA_FILE))?;
    for entry in fs::read_dir(path).ok().into_iter().flatten() {
        let path = entry?.path();
        if path.is_dir() {
            return recurse_mcp_tools_dirs(root, &path, tools);
        }
        if path.extension().is_none_or(|ext| ext != "toml") {
            continue;
        }
        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = filename.strip_suffix(".toml") else {
            continue;
        };

        let name = path
            .parent()
            .unwrap_or(&path)
            .strip_prefix(root)
            .unwrap_or(&path)
            .join(name)
            .iter()
            .filter_map(OsStr::to_str)
            .collect::<Vec<_>>()
            .join("_");

        let contents = fs::read_to_string(path)?;
        let mut contents: toml::Table = toml::from_str(&contents)?;

        let command = contents
            .get("command")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str());

        if let Some(template) = contents
            .get("inherit")
            .and_then(toml::Value::as_str)
            .and_then(|s| metadata.templates.get(s))
        {
            contents.insert(
                "command".to_owned(),
                template
                    .command
                    .iter()
                    .map(String::as_str)
                    .chain(command)
                    .collect::<Vec<_>>()
                    .into(),
            );
        }

        let mut tool: McpTool = contents.try_into()?;
        tool.id = McpToolId::new(name.clone());

        tools.insert(McpToolId::new(name), tool);
    }

    Ok(())
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

fn remove_unused_conversation_dirs(
    id: &ConversationId,
    conversation_dir: &Path,
    workspace_conversations_dir: &Path,
    user_conversations_dir: &Path,
) -> Result<()> {
    // Gather all possible conversation directory names
    let mut dirs = vec![];
    for conversations_dir in &[workspace_conversations_dir, user_conversations_dir] {
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

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        str::FromStr as _,
    };

    use jp_conversation::ConversationId;
    use jp_mcp::transport::{self, Transport};
    use tempfile::tempdir;
    use test_log::test;

    use super::*;

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
    fn test_load_mcp_servers() {
        let original_dir = tempdir().unwrap();
        let storage = Storage::new(original_dir.path()).unwrap();
        let mcp_path = storage.root.join(MCP_SERVERS_DIR);
        fs::create_dir_all(&mcp_path).unwrap();

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

        storage
            .persist_mcp_servers(&TombMap::from([(id.clone(), server.clone())]))
            .unwrap();

        let servers_path = root.join(MCP_SERVERS_DIR);
        assert!(servers_path.is_dir());
        assert!(servers_path.join(format!("{id}.json")).is_file());

        let storage = Storage::new(root).unwrap();
        let servers = storage.load_mcp_servers().unwrap();
        assert_eq!(servers.len(), 1);
        assert!(servers.contains_key(&id));
    }
}
