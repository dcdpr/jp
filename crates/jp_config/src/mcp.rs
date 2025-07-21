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
    #[serde(default)]
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

        let defaults = self.servers.get(&global_id).cloned().unwrap_or_default();
        let server = self.servers.get(&id);

        Server {
            enable: server
                .filter(|s| s.enable != defaults.enable)
                .unwrap_or(&defaults)
                .enable,

            binary_checksum: server
                .filter(|s| s.binary_checksum != defaults.binary_checksum)
                .unwrap_or(&defaults)
                .binary_checksum
                .clone(),

            tools: server
                .filter(|s| s.tools != defaults.tools)
                .unwrap_or(&defaults)
                .tools
                .clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::server::{
        checksum::{Algorithm, Checksum},
        tool::Tool,
        ToolId,
    };

    #[test]
    #[expect(clippy::too_many_lines)]
    fn test_mcp_get_server_with_defaults() {
        struct TestCase {
            servers: Vec<(&'static str, Server)>,
            input: &'static str,
            expected: Server,
        }

        let cases = vec![
            ("no servers", TestCase {
                servers: vec![],
                input: "foo",
                expected: Server {
                    enable: true,
                    binary_checksum: None,
                    tools: ConfigMap::default(),
                },
            }),
            ("no match", TestCase {
                servers: vec![("foo", Server {
                    enable: false,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "foo".to_owned(),
                    }),
                    tools: ConfigMap::default(),
                })],
                input: "bar",
                expected: Server {
                    enable: true,
                    binary_checksum: None,
                    tools: ConfigMap::default(),
                },
            }),
            ("single match", TestCase {
                servers: vec![("foo", Server {
                    enable: false,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "bar".to_owned(),
                    }),
                    tools: ConfigMap::default(),
                })],
                input: "foo",
                expected: Server {
                    enable: false,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "bar".to_owned(),
                    }),
                    tools: ConfigMap::default(),
                },
            }),
            ("global defaults only", TestCase {
                servers: vec![("*", Server {
                    enable: false,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "global".to_owned(),
                    }),
                    tools: ConfigMap::from_iter(vec![(ToolId::new("*"), Tool {
                        enable: !Tool::default().enable,
                        ..Tool::default()
                    })]),
                })],
                input: "nonexistent",
                expected: Server {
                    enable: false,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "global".to_owned(),
                    }),
                    tools: ConfigMap::from_iter(vec![(ToolId::new("*"), Tool {
                        enable: !Tool::default().enable,
                        ..Tool::default()
                    })]),
                },
            }),
            ("merge with global defaults - full override", TestCase {
                servers: vec![
                    ("*", Server {
                        enable: false,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "default".to_owned(),
                        }),
                        tools: ConfigMap::from_iter(vec![(ToolId::new("default_tool"), Tool {
                            enable: !Tool::default().enable,
                            ..Tool::default()
                        })]),
                    }),
                    ("specific", Server {
                        enable: true,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "specific".to_owned(),
                        }),
                        tools: ConfigMap::from_iter(vec![(
                            ToolId::new("specific_tool"),
                            Tool::default(),
                        )]),
                    }),
                ],
                input: "specific",
                expected: Server {
                    enable: true,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "specific".to_owned(),
                    }),
                    tools: ConfigMap::from_iter(vec![(
                        ToolId::new("specific_tool"),
                        Tool::default(),
                    )]),
                },
            }),
            (
                "merge with global defaults - partial override enable only",
                TestCase {
                    servers: vec![
                        ("*", Server {
                            enable: false,
                            binary_checksum: Some(Checksum {
                                algorithm: Algorithm::Sha256,
                                value: "default".to_owned(),
                            }),
                            tools: ConfigMap::from_iter(vec![(
                                ToolId::new("default_tool"),
                                Tool {
                                    enable: !Tool::default().enable,
                                    ..Tool::default()
                                },
                            )]),
                        }),
                        ("specific", Server {
                            enable: true,
                            binary_checksum: Some(Checksum {
                                algorithm: Algorithm::Sha256,
                                value: "default".to_owned(),
                            }),
                            tools: ConfigMap::from_iter(vec![(
                                ToolId::new("default_tool"),
                                Tool {
                                    enable: !Tool::default().enable,
                                    ..Tool::default()
                                },
                            )]),
                        }),
                    ],
                    input: "specific",
                    expected: Server {
                        enable: true,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "default".to_owned(),
                        }),
                        tools: ConfigMap::from_iter(vec![(ToolId::new("default_tool"), Tool {
                            enable: !Tool::default().enable,
                            ..Tool::default()
                        })]),
                    },
                },
            ),
            (
                "merge with global defaults - partial override checksum only",
                TestCase {
                    servers: vec![
                        ("*", Server {
                            enable: false,
                            binary_checksum: Some(Checksum {
                                algorithm: Algorithm::Sha256,
                                value: "default".to_owned(),
                            }),
                            tools: ConfigMap::from_iter(vec![(
                                ToolId::new("default_tool"),
                                Tool {
                                    enable: !Tool::default().enable,
                                    ..Tool::default()
                                },
                            )]),
                        }),
                        ("specific", Server {
                            enable: false,
                            binary_checksum: Some(Checksum {
                                algorithm: Algorithm::Sha256,
                                value: "specific".to_owned(),
                            }),
                            tools: ConfigMap::from_iter(vec![(
                                ToolId::new("default_tool"),
                                Tool {
                                    enable: !Tool::default().enable,
                                    ..Tool::default()
                                },
                            )]),
                        }),
                    ],
                    input: "specific",
                    expected: Server {
                        enable: false,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "specific".to_owned(),
                        }),
                        tools: ConfigMap::from_iter(vec![(ToolId::new("default_tool"), Tool {
                            enable: !Tool::default().enable,
                            ..Tool::default()
                        })]),
                    },
                },
            ),
            (
                "merge with global defaults - partial override tools only",
                TestCase {
                    servers: vec![
                        ("*", Server {
                            enable: false,
                            binary_checksum: Some(Checksum {
                                algorithm: Algorithm::Sha256,
                                value: "default".to_owned(),
                            }),
                            tools: ConfigMap::from_iter(vec![(
                                ToolId::new("default_tool"),
                                Tool {
                                    enable: !Tool::default().enable,
                                    ..Tool::default()
                                },
                            )]),
                        }),
                        ("specific", Server {
                            enable: false,
                            binary_checksum: Some(Checksum {
                                algorithm: Algorithm::Sha256,
                                value: "default".to_owned(),
                            }),
                            tools: ConfigMap::from_iter(vec![(
                                ToolId::new("specific_tool"),
                                Tool::default(),
                            )]),
                        }),
                    ],
                    input: "specific",
                    expected: Server {
                        enable: false,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "default".to_owned(),
                        }),
                        tools: ConfigMap::from_iter(vec![(
                            ToolId::new("specific_tool"),
                            Tool::default(),
                        )]),
                    },
                },
            ),
            ("merge None checksum with Some default", TestCase {
                servers: vec![
                    ("*", Server {
                        enable: true,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "default".to_owned(),
                        }),
                        tools: ConfigMap::default(),
                    }),
                    ("specific", Server {
                        enable: true,
                        binary_checksum: None,
                        tools: ConfigMap::default(),
                    }),
                ],
                input: "specific",
                expected: Server {
                    enable: true,
                    binary_checksum: None,
                    tools: ConfigMap::default(),
                },
            }),
            ("merge Some checksum with None default", TestCase {
                servers: vec![
                    ("*", Server {
                        enable: true,
                        binary_checksum: None,
                        tools: ConfigMap::default(),
                    }),
                    ("specific", Server {
                        enable: true,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "specific".to_owned(),
                        }),
                        tools: ConfigMap::default(),
                    }),
                ],
                input: "specific",
                expected: Server {
                    enable: true,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "specific".to_owned(),
                    }),
                    tools: ConfigMap::default(),
                },
            }),
            ("exact match with defaults should use defaults", TestCase {
                servers: vec![
                    ("*", Server {
                        enable: false,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "same".to_owned(),
                        }),
                        tools: ConfigMap::from_iter(vec![(ToolId::new("tool"), Tool {
                            enable: !Tool::default().enable,
                            ..Tool::default()
                        })]),
                    }),
                    ("specific", Server {
                        enable: false,
                        binary_checksum: Some(Checksum {
                            algorithm: Algorithm::Sha256,
                            value: "same".to_owned(),
                        }),
                        tools: ConfigMap::from_iter(vec![(ToolId::new("tool"), Tool {
                            enable: !Tool::default().enable,
                            ..Tool::default()
                        })]),
                    }),
                ],
                input: "specific",
                expected: Server {
                    enable: false,
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "same".to_owned(),
                    }),
                    tools: ConfigMap::from_iter(vec![(ToolId::new("tool"), Tool {
                        enable: !Tool::default().enable,
                        ..Tool::default()
                    })]),
                },
            }),
        ];

        for (name, test) in cases {
            let mcp = Mcp {
                servers: test
                    .servers
                    .into_iter()
                    .map(|(k, v)| (ServerId(k.to_string()), v))
                    .collect(),
            };

            let received = mcp.get_server_with_defaults(test.input);
            dbg!(&received);
            assert_eq!(received, test.expected, "test case: {name}");
        }
    }
}
