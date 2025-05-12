pub mod title;

use confique::Config as Confique;
use jp_conversation::{ContextId, PersonaId};
use jp_mcp::config::McpServerId;
use serde::Deserialize as _;

use crate::{error::Result, parse_vec};

/// LLM configuration.
#[derive(Debug, Clone, Default, Confique)]
pub struct Config {
    /// Title configuration.
    #[config(nested)]
    pub title: title::Config,

    /// Persona to use for the active conversation.
    ///
    /// If unset, uses the `default` persona, if one exists.
    #[config(env = "JP_CONVERSATION_PERSONA", deserialize_with = de_persona)]
    pub persona: Option<PersonaId>,

    /// Context to use for the active conversation.
    ///
    /// If unset, uses the `default` context, if one exists.
    #[config(env = "JP_CONVERSATION_CONTEXT", deserialize_with = de_context)]
    pub context: Option<ContextId>,

    /// List of MCP servers to use for the active conversation.
    #[config(default = [], env = "JP_CONVERSATION_MCP_SERVERS", deserialize_with = de_mcp_servers)]
    pub mcp_servers: Vec<McpServerId>,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            _ if key.starts_with("title.") => self.title.set(path, &key[6..], value)?,
            "persona" => self.persona = Some(value.into().parse()?),
            "context" => self.context = Some(value.into().parse()?),
            "mcp_servers" => {
                self.mcp_servers = parse_vec(&value.into(), McpServerId::new);
            }
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

pub fn de_persona<'de, D>(deserializer: D) -> std::result::Result<PersonaId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer)
        .and_then(|s| PersonaId::try_from(s).map_err(serde::de::Error::custom))
}

pub fn de_context<'de, D>(deserializer: D) -> std::result::Result<ContextId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(deserializer)
        .and_then(|s| ContextId::try_from(s).map_err(serde::de::Error::custom))
}

pub fn de_mcp_servers<'de, D>(deserializer: D) -> std::result::Result<Vec<McpServerId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Vec::<String>::deserialize(deserializer)?
        .into_iter()
        .map(McpServerId::new)
        .collect())
}
