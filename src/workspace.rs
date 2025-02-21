pub mod message;
pub mod session;

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use exodus_trace::info;
use session::Session;
use uuid::Uuid;

use crate::{
    openrouter::{ChatMessage, Role},
    Message, WorkspaceSessions,
};

pub const WORKSPACE_DIR: &str = ".jp";
pub const WORKSPACE_CONFIG: &str = ".jp.toml";

#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub active_session: Uuid,
    pub sessions: Vec<Session>,
    pub messages: Vec<Message>,
}

impl Workspace {
    pub fn load() -> Result<Self> {
        let Some(root) = find_root(&env::current_dir()?) else {
            bail!("Cannot load workspace, not in a workspace context.")
        };
        let WorkspaceSessions {
            active_id,
            sessions,
        } = WorkspaceSessions::load(&root)?;

        let messages = Message::load_all(&root, &active_id)?;

        Ok(Self {
            root,
            active_session: active_id,
            sessions,
            messages,
        })
    }

    pub fn chat_history(&self) -> impl Iterator<Item = ChatMessage> {
        self.messages.clone().into_iter().flat_map(|msg| {
            vec![
                ChatMessage {
                    role: Role::User,
                    content: msg.query.clone(),
                },
                ChatMessage {
                    role: Role::Assistant,
                    content: msg.response.clone(),
                },
            ]
        })
    }
}

/// Initialize workspace state for a new workspace
pub fn initialize_workspace_state(root: &Path) -> Result<()> {
    let jp_dir = root.join(WORKSPACE_DIR);

    if !jp_dir.exists() {
        fs::create_dir_all(&jp_dir).context(format!(
            "Failed to create workspace directory at {:?}",
            jp_dir
        ))?;
        info!("Created workspace directory at {:?}", jp_dir);
    }

    info!("Initializing workspace sessions");
    session::initialize_workspace_sessions(root)?;

    Ok(())
}

/// Find a workspace root by traversing up from the current directory
pub fn find_root(starting_dir: &Path) -> Option<PathBuf> {
    let mut current_dir = starting_dir.to_path_buf();

    loop {
        let config_path = current_dir.join(WORKSPACE_CONFIG);
        if config_path.exists() {
            return Some(current_dir);
        }

        if !current_dir.pop() {
            return None;
        }
    }
}
