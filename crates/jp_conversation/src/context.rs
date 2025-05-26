//! Defines the context for a conversation or message.

use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::PathBuf,
    str::FromStr,
};

use jp_id::{
    parts::{GlobalId, TargetId, Variant},
    Id,
};
use jp_mcp::{config::McpServerId, tool::ToolChoice};
use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    PersonaId,
};

/// Attachments + persona for a specific query
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Context {
    pub persona_id: PersonaId,

    #[serde(
        default,
        rename = "mcp_servers",
        skip_serializing_if = "HashSet::is_empty"
    )]
    pub mcp_server_ids: HashSet<McpServerId>,

    #[serde(
        default,
        rename = "attachments",
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub attachment_handlers: HashMap<String, jp_attachment::BoxedHandler>,

    /// How the assistant should choose tools, if any are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
}

impl Context {
    #[must_use]
    pub fn new(persona_id: PersonaId) -> Self {
        Self {
            persona_id,
            ..Default::default()
        }
    }
}

/// ID wrapper for Context
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContextId(String);

impl ContextId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        format!("{}.json", self.target_id()).into()
    }

    pub fn from_filename(filename: &str) -> Result<Self> {
        filename
            .strip_suffix(".json")
            .ok_or_else(|| Error::InvalidIdFormat(format!("Invalid context filename: {filename}")))
            .and_then(Self::try_from)
    }
}

impl Default for ContextId {
    fn default() -> Self {
        Self("default".to_owned())
    }
}

impl Id for ContextId {
    fn variant() -> Variant {
        'c'.into()
    }

    fn target_id(&self) -> TargetId {
        self.0.clone().into()
    }

    fn global_id(&self) -> GlobalId {
        jp_id::global::get().into()
    }

    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.global_id().is_valid()
    }
}

impl fmt::Display for ContextId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<&str> for ContextId {
    type Error = Error;

    fn try_from(s: &str) -> Result<Self> {
        Self::try_from(s.to_owned())
    }
}

impl TryFrom<&String> for ContextId {
    type Error = Error;

    fn try_from(s: &String) -> Result<Self> {
        Self::try_from(s.as_str())
    }
}

impl TryFrom<String> for ContextId {
    type Error = Error;

    fn try_from(s: String) -> Result<Self> {
        if s.chars().any(|c| {
            !(c.is_numeric()
                || (c.is_ascii_alphabetic() && c.is_ascii_lowercase())
                || c == '-'
                || c == '_')
        }) {
            return Err(Error::InvalidIdFormat(
                "Persona ID must be [a-z0-9_-]".to_string(),
            ));
        }

        Ok(Self(s))
    }
}

impl FromStr for ContextId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        jp_id::parse::<Self>(s)
            .map(|p| Self(p.target_id.to_string()))
            .map_err(Into::into)
    }
}
