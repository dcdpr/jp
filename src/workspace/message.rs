use std::{fs, path::Path};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::WORKSPACE_DIR;

const MESSAGES_DIR: &str = "messages";

// Structure representing a single message exchange
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Message {
    pub id: Uuid,
    pub created_at: DateTime<Utc>,
    pub query: String,
    pub reasoning: Option<String>,
    pub response: String,
}

impl Message {
    /// Create a new message
    pub fn new(query: &str, reasoning: Option<String>, response: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            created_at: Utc::now(),
            query: query.to_owned(),
            reasoning,
            response,
        }
    }

    /// Save a message to the filesystem
    pub fn save(&self, project_root: &Path, session_id: &Uuid) -> Result<()> {
        let messages_dir = project_root
            .join(WORKSPACE_DIR)
            .join(MESSAGES_DIR)
            .join(session_id.to_string());

        // Ensure directory exists
        if !messages_dir.exists() {
            fs::create_dir_all(&messages_dir).context(format!(
                "Failed to create messages directory at {:?}",
                messages_dir
            ))?;
        }

        // Format timestamp for filename
        let timestamp = self.created_at.format("%Y%m%dT%H%M%S");
        let filename = format!("{}-{}.jsonc", timestamp, self.id);
        let file_path = messages_dir.join(filename);

        // Serialize with pretty formatting
        let json = serde_json::to_string_pretty(self).context("Failed to serialize message")?;

        fs::write(&file_path, json)
            .context(format!("Failed to write message file to {:?}", file_path))?;

        Ok(())
    }

    /// Load all messages for a specific scope
    pub fn load_all(workspace_root: &Path, session_id: &Uuid) -> Result<Vec<Self>> {
        let messages_dir = workspace_root
            .join(WORKSPACE_DIR)
            .join("messages")
            .join(session_id.to_string());

        if !messages_dir.exists() {
            return Ok(Vec::new());
        }

        let mut messages = Vec::new();

        for entry in fs::read_dir(messages_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "jsonc") {
                let json = fs::read_to_string(&path)
                    .context(format!("Failed to read message file from {:?}", path))?;

                let message: Self = serde_json::from_str(&json)
                    .context(format!("Failed to parse message JSON from {:?}", path))?;

                messages.push(message);
            }
        }

        // Sort messages by creation time
        messages.sort_by_key(|msg| msg.created_at);

        Ok(messages)
    }
}
