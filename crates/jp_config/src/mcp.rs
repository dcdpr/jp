pub mod server;
pub mod tool_call;

use std::ops::Deref;

use confique::{
    internal::map_err_prefix_path,
    meta::{Field, FieldKind, Meta},
    Config as Confique, Partial,
};
use serde::{Deserialize, Serialize};
use server::{Server, ServerPartial};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    map::{ConfigKey, ConfigMap, ConfigMapPartial},
    Error,
};

/// MCP configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mcp {
    /// MCP servers configuration.
    ///
    /// # Server Defaults
    ///
    /// A special `*` server ID can be used to set defaults for all servers. Any
    /// other server IDs will override the defaults.
    pub servers: ConfigMap<ServerId, Server>,
}

impl AssignKeyValue for <Mcp as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<(), Error> {
        let k = kv.key().as_str().to_owned();

        match k.as_str() {
            "servers" => self.servers = kv.try_into_object()?,

            _ if kv.trim_prefix("servers") => {
                self.servers
                    .entry(ServerId(
                        kv.key_mut().trim_any_prefix().ok_or(set_error(kv.key()))?,
                    ))
                    .or_insert(ServerPartial::default_values())
                    .assign(kv)?;
            }

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ServerId(String);

impl Deref for ServerId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ConfigKey for ServerId {
    const KIND: &'static str = "<server_id>";
}

impl Confique for Mcp {
    type Partial = McpPartial;

    const META: Meta = Meta {
        name: "mcp",
        doc: &[],
        fields: &[Field {
            name: "servers",
            doc: &[],
            kind: FieldKind::Nested {
                meta: &ConfigMap::<ServerId, Server>::META,
            },
        }],
    };

    fn from_partial(partial: Self::Partial) -> Result<Self, confique::Error> {
        Ok(Self {
            servers: map_err_prefix_path(ConfigMap::from_partial(partial.servers), "servers")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpPartial {
    pub servers: ConfigMapPartial<ServerId, ServerPartial>,
}

impl Default for McpPartial {
    fn default() -> Self {
        let mut servers = ConfigMapPartial::default();
        servers.insert(ServerId("*".to_string()), ServerPartial::default());

        Self { servers }
    }
}

impl Partial for McpPartial {
    fn empty() -> Self {
        Self {
            servers: ConfigMapPartial::empty(),
        }
    }

    fn default_values() -> Self {
        Self::default()
    }

    fn from_env() -> Result<Self, confique::Error> {
        Ok(Self {
            servers: ConfigMapPartial::from_env()?,
        })
    }

    fn with_fallback(self, fallback: Self) -> Self {
        Self {
            servers: self.servers.with_fallback(fallback.servers),
        }
    }

    fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    fn is_complete(&self) -> bool {
        self.servers.is_complete()
    }
}

impl Mcp {
    /// Get a server by ID.
    ///
    /// This handles fetching defaults from any `*` server/tool as well.
    #[must_use]
    pub fn get_server_with_defaults(&self, id: &str) -> Server {
        let global_id = ServerId(String::from("*"));
        let id = ServerId(id.to_owned());

        self.servers
            .get(&id)
            .cloned()
            .or_else(|| self.servers.get(&global_id).cloned())
            .unwrap_or(Server::default())
    }
}
