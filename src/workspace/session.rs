use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use exodus_trace::info;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::WORKSPACE_DIR;

// Main structure representing all sessions in a workspace
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WorkspaceSessions {
    pub active_id: Uuid,
    pub sessions: Vec<Session>,
}

// Structure representing a single conversation session
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Session {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

// Constants
const SESSIONS_FILE: &str = "sessions.jsonc";

impl WorkspaceSessions {
    /// Initialize project sessions for a new project
    pub fn initialize(workspace_root: &Path) -> Result<Self> {
        let jp_dir = workspace_root.join(WORKSPACE_DIR);
        if !jp_dir.exists() {
            fs::create_dir_all(&jp_dir)
                .context(format!("Failed to create directory at {:?}", jp_dir))?;
        }

        // Create initial session
        let initial_session_id = Uuid::new_v4();
        let now = Utc::now();

        let sessions = Self {
            active_id: initial_session_id,
            sessions: vec![Session {
                id: initial_session_id,
                created_at: now,
                updated_at: now,
            }],
        };

        // Save to file
        sessions.save(workspace_root)?;

        // Create messages directory for the initial scope
        let messages_dir = jp_dir.join("messages").join(initial_session_id.to_string());
        fs::create_dir_all(messages_dir)
            .context("Failed to create messages directory for initial session")?;

        Ok(sessions)
    }

    /// Save workspace sessions to the sessions file
    pub fn save(&self, workspace_root: &Path) -> Result<()> {
        let scopes_path = workspace_root.join(WORKSPACE_DIR).join(SESSIONS_FILE);

        // Serialize with pretty formatting
        let json =
            serde_json::to_string_pretty(self).context("Failed to serialize project scopes")?;

        fs::write(&scopes_path, json)
            .context(format!("Failed to write scopes file to {:?}", scopes_path))?;

        Ok(())
    }

    /// Load workspace sessions from a workspace directory
    pub fn load(workspace_root: &Path) -> Result<Self> {
        let sessions_path = workspace_root.join(WORKSPACE_DIR).join(SESSIONS_FILE);

        if !sessions_path.exists() {
            bail!("Sessions file not found at {:?}", sessions_path);
        }

        let json = fs::read_to_string(&sessions_path).context(format!(
            "Failed to read sessions file from {:?}",
            sessions_path
        ))?;

        serde_json::from_str(&json).context("Failed to parse sessions JSON")
    }
}

pub fn initialize_workspace_sessions(project_root: &Path) -> Result<()> {
    // Check if scopes already exist
    let scopes_path = project_root.join(WORKSPACE_DIR).join(SESSIONS_FILE);

    if scopes_path.exists() {
        info!("Project scopes already initialized at {:?}", project_root);
        return Ok(());
    }

    // Initialize new project scopes
    info!("Initializing project scopes at {:?}", project_root);
    WorkspaceSessions::initialize(project_root)?;

    Ok(())
}
