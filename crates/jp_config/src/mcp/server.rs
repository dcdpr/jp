pub mod checksum;
pub mod tool;

use std::ops::Deref;

use checksum::{Checksum, ChecksumPartial};
use confique::{
    internal::map_err_prefix_path,
    meta::{Field, FieldKind, LeafKind, Meta},
    Config as Confique, Partial,
};
use serde::{Deserialize, Serialize};
use tool::{Tool, ToolPartial};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    map::{ConfigKey, ConfigMap, ConfigMapPartial},
    Error,
};

/// MCP server configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Server {
    /// Whether to enable the MCP server.
    pub enable: bool,

    /// Tools configurations for the MCP server.
    ///
    /// # Tool Defaults
    ///
    /// A special `*` tool ID can be used to set defaults for all tools of the
    /// given server. Any other tool IDs will override the defaults.
    ///
    /// # Tool Uniqueness
    ///
    /// Note that tool names must be unique across all MCP servers. However, if
    /// a tool name is already taken by a tool in another MCP server, the server
    /// ID will be prepended to the tool name, separated by an underscore.
    ///
    /// Meaning, the first server's tool named `foo` will be named as-is, but
    /// subsequent servers' tools named `foo` will be named `<server_id>_foo`.
    pub tools: ConfigMap<ToolId, Tool>,

    /// The checksum for the MCP server binary.
    pub binary_checksum: Option<Checksum>,
}

impl AssignKeyValue for <Server as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<(), Error> {
        let k = kv.key().as_str().to_owned();

        match k.as_str() {
            "enable" => self.enable = Some(kv.try_into_bool()?),
            "tools" => self.tools = kv.try_into_object()?,
            "binary_checksum" => self.binary_checksum = kv.try_into_object()?,

            _ if kv.trim_prefix("tools") => {
                self.tools
                    .entry(ToolId(
                        kv.key_mut().trim_any_prefix().ok_or(set_error(kv.key()))?,
                    ))
                    .or_insert(ToolPartial::default_values())
                    .assign(kv)?;
            }
            _ if kv.trim_prefix("binary_checksum") => self
                .binary_checksum
                .get_or_insert(ChecksumPartial::default_values())
                .assign(kv)?,

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}

impl Confique for Server {
    type Partial = ServerPartial;

    const META: Meta = Meta {
        name: "server",
        doc: &[],
        fields: &[
            Field {
                name: "enable",
                doc: &[],
                kind: FieldKind::Leaf {
                    env: None,
                    kind: LeafKind::Required { default: None },
                },
            },
            Field {
                name: "tools",
                doc: &[],
                kind: FieldKind::Nested {
                    meta: &ConfigMap::<ToolId, Tool>::META,
                },
            },
            Field {
                name: "binary_checksum",
                doc: &[],
                kind: FieldKind::Nested {
                    meta: &Checksum::META,
                },
            },
        ],
    };

    fn from_partial(partial: Self::Partial) -> Result<Self, confique::Error> {
        Ok(Self {
            enable: partial.enable.unwrap_or(true),
            tools: map_err_prefix_path(ConfigMap::from_partial(partial.tools), "tools")?,
            binary_checksum: partial
                .binary_checksum
                .map(Checksum::from_partial)
                .transpose()?,
        })
    }
}

impl Server {
    /// Get a tool by ID.
    ///
    /// This handles fetching defaults from any `*` tool as well.
    #[must_use]
    pub fn get_tool_with_defaults(&self, id: &str) -> Tool {
        let global_id = ToolId(String::from("*"));
        let id = ToolId(id.to_owned());

        self.tools
            .get(&id)
            .cloned()
            .or_else(|| self.tools.get(&global_id).cloned())
            .unwrap_or(Tool::default())
    }
}

impl Default for Server {
    fn default() -> Self {
        Self {
            enable: true,
            tools: ConfigMap::default(),
            binary_checksum: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolId(String);

impl Deref for ToolId {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ConfigKey for ToolId {
    const KIND: &'static str = "<tool_id>";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerPartial {
    pub enable: Option<bool>,
    pub tools: ConfigMapPartial<ToolId, ToolPartial>,
    pub binary_checksum: Option<checksum::ChecksumPartial>,
}

impl Default for ServerPartial {
    fn default() -> Self {
        let mut tools = ConfigMapPartial::default();
        tools.insert(ToolId("*".to_string()), ToolPartial::default());

        Self {
            enable: Some(true),
            tools,
            binary_checksum: None,
        }
    }
}

impl Partial for ServerPartial {
    fn empty() -> Self {
        Self {
            enable: None,
            tools: ConfigMapPartial::empty(),
            binary_checksum: None,
        }
    }

    fn default_values() -> Self {
        Self::default()
    }

    fn from_env() -> Result<Self, confique::Error> {
        unimplemented!("use jp_config::Config::set_from_envs() instead")
    }

    fn with_fallback(self, fallback: Self) -> Self {
        Self {
            enable: self.enable.or(fallback.enable),
            tools: self.tools.with_fallback(fallback.tools),
            binary_checksum: self.binary_checksum.or(fallback.binary_checksum),
        }
    }

    fn is_empty(&self) -> bool {
        self.enable.is_none() && self.tools.is_empty() && self.binary_checksum.is_none()
    }

    fn is_complete(&self) -> bool {
        self.enable.is_some()
            && self.tools.is_complete()
            && self
                .binary_checksum
                .as_ref()
                .is_some_and(Partial::is_complete)
    }
}
