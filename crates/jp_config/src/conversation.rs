pub mod title;

use confique::Config as Confique;
use jp_mcp::config::McpServerId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    is_default,
};

/// LLM configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Conversation {
    /// Title configuration.
    #[config(nested)]
    pub title: title::Title,

    /// List of MCP servers to use for the active conversation.
    #[config(
        default = [],
        deserialize_with = de_mcp_servers,
        partial_attr(serde(skip_serializing_if = "is_default")),
    )]
    pub mcp_servers: Vec<McpServerId>,

    #[config(default = [], partial_attr(serde(skip_serializing_if = "is_default")))]
    pub attachments: Vec<url::Url>,
}

impl AssignKeyValue for <Conversation as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();

        match k.as_str() {
            "title" => self.title = kv.try_into_object()?,
            "mcp_servers" => {
                kv.try_set_or_merge_vec(self.mcp_servers.get_or_insert_default(), |v| match v {
                    Value::String(v) => Ok(McpServerId::new(v)),
                    _ => Err("Expected string".into()),
                })?;
            }
            "attachments" => {
                kv.try_set_or_merge_vec(self.attachments.get_or_insert_default(), |v| match v {
                    Value::String(v) => Ok(url::Url::parse(&v)?),
                    _ => Err("Expected string".into()),
                })?;
            }

            _ if kv.trim_prefix("title") => self.title.assign(kv)?,

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
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
