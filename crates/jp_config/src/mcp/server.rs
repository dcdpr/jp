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

impl Server {
    #[must_use]
    pub fn get_tool(&self, id: &ToolId) -> Tool {
        self.tools
            .get(id)
            .cloned()
            .or_else(|| self.tools.get(&ToolId::new("*")).cloned())
            .unwrap_or_default()
    }
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

        let defaults = self.tools.get(&global_id).cloned().unwrap_or_default();
        let tool = self.tools.get(&id);

        Tool {
            enable: tool
                .filter(|s| s.enable != defaults.enable)
                .unwrap_or(&defaults)
                .enable,

            run: tool
                .filter(|s| s.run != defaults.run)
                .unwrap_or(&defaults)
                .run,

            result: tool
                .filter(|s| s.result != defaults.result)
                .unwrap_or(&defaults)
                .result,

            style: tool
                .filter(|s| s.style != defaults.style)
                .unwrap_or(&defaults)
                .style
                .clone(),
        }
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

impl ToolId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,
    #[serde(default, skip_serializing_if = "ConfigMapPartial::is_empty")]
    pub tools: ConfigMapPartial<ToolId, ToolPartial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_checksum: Option<checksum::ChecksumPartial>,
}

impl ServerPartial {
    #[must_use]
    pub(crate) fn get_tool_or_empty(&self, tool_id: &ToolId) -> ToolPartial {
        self.tools
            .get(tool_id)
            .cloned()
            .unwrap_or(ToolPartial::empty())
    }
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
            binary_checksum: self.binary_checksum.or(fallback.binary_checksum),
            tools: self.tools.with_fallback(fallback.tools),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mcp::{
            server::{
                tool::{ResultMode, RunMode, Tool},
                ToolId,
            },
            tool_call::{InlineResults, ToolCall},
        },
        style::LinkStyle,
    };

    #[test]
    #[expect(clippy::too_many_lines)]
    fn test_server_get_tool_with_defaults() {
        struct TestCase {
            tools: Vec<(&'static str, Tool)>,
            input: &'static str,
            expected: Tool,
        }

        let cases = vec![
            ("no tools", TestCase {
                tools: vec![],
                input: "foo",
                expected: Tool::default(),
            }),
            ("truncated inline results merge test", TestCase {
                tools: vec![
                    ("*", Tool {
                        enable: true,
                        run: RunMode::Ask,
                        result: ResultMode::Always,
                        style: ToolCall {
                            inline_results: InlineResults::Truncate { lines: 5 },
                            results_file_link: LinkStyle::Osc8,
                        },
                    }),
                    ("specific", Tool {
                        enable: true,
                        run: RunMode::Ask,
                        result: ResultMode::Always,
                        style: ToolCall {
                            inline_results: InlineResults::Truncate { lines: 20 },
                            results_file_link: LinkStyle::Osc8,
                        },
                    }),
                ],
                input: "specific",
                expected: Tool {
                    enable: true,
                    run: RunMode::Ask,
                    result: ResultMode::Always,
                    style: ToolCall {
                        inline_results: InlineResults::Truncate { lines: 20 },
                        results_file_link: LinkStyle::Osc8,
                    },
                },
            }),
            ("no match", TestCase {
                tools: vec![("foo", Tool {
                    enable: false,
                    ..Tool::default()
                })],
                input: "bar",
                expected: Tool::default(),
            }),
            ("single match", TestCase {
                tools: vec![("foo", Tool {
                    enable: false,
                    ..Tool::default()
                })],
                input: "foo",
                expected: Tool {
                    enable: false,
                    ..Tool::default()
                },
            }),
            ("global defaults only", TestCase {
                tools: vec![("*", Tool {
                    enable: false,
                    ..Tool::default()
                })],
                input: "nonexistent",
                expected: Tool {
                    enable: false,
                    ..Tool::default()
                },
            }),
            ("merge with global defaults - full override", TestCase {
                tools: vec![
                    ("*", Tool {
                        enable: false,
                        run: RunMode::Edit,
                        result: ResultMode::Ask,
                        style: ToolCall {
                            inline_results: InlineResults::Off,
                            results_file_link: LinkStyle::Off,
                        },
                    }),
                    ("specific", Tool {
                        enable: true,
                        run: RunMode::Always,
                        result: ResultMode::Edit,
                        style: ToolCall {
                            inline_results: InlineResults::Truncate { lines: 10 },
                            results_file_link: LinkStyle::Full,
                        },
                    }),
                ],
                input: "specific",
                expected: Tool {
                    enable: true,
                    run: RunMode::Always,
                    result: ResultMode::Edit,
                    style: ToolCall {
                        inline_results: InlineResults::Truncate { lines: 10 },
                        results_file_link: LinkStyle::Full,
                    },
                },
            }),
            (
                "merge with global defaults - partial override enable only",
                TestCase {
                    tools: vec![
                        ("*", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                        ("specific", Tool {
                            enable: true,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                    ],
                    input: "specific",
                    expected: Tool {
                        enable: true,
                        run: RunMode::Edit,
                        result: ResultMode::Ask,
                        style: ToolCall {
                            inline_results: InlineResults::Off,
                            results_file_link: LinkStyle::Off,
                        },
                    },
                },
            ),
            (
                "merge with global defaults - partial override run only",
                TestCase {
                    tools: vec![
                        ("*", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                        ("specific", Tool {
                            enable: false,
                            run: RunMode::Always,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                    ],
                    input: "specific",
                    expected: Tool {
                        enable: false,
                        run: RunMode::Always,
                        result: ResultMode::Ask,
                        style: ToolCall {
                            inline_results: InlineResults::Off,
                            results_file_link: LinkStyle::Off,
                        },
                    },
                },
            ),
            (
                "merge with global defaults - partial override result only",
                TestCase {
                    tools: vec![
                        ("*", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                        ("specific", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Edit,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                    ],
                    input: "specific",
                    expected: Tool {
                        enable: false,
                        run: RunMode::Edit,
                        result: ResultMode::Edit,
                        style: ToolCall {
                            inline_results: InlineResults::Off,
                            results_file_link: LinkStyle::Off,
                        },
                    },
                },
            ),
            (
                "merge with global defaults - partial override style inline_results only",
                TestCase {
                    tools: vec![
                        ("*", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                        ("specific", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Full,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                    ],
                    input: "specific",
                    expected: Tool {
                        enable: false,
                        run: RunMode::Edit,
                        result: ResultMode::Ask,
                        style: ToolCall {
                            inline_results: InlineResults::Full,
                            results_file_link: LinkStyle::Off,
                        },
                    },
                },
            ),
            (
                "merge with global defaults - partial override style results_file_link only",
                TestCase {
                    tools: vec![
                        ("*", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Off,
                            },
                        }),
                        ("specific", Tool {
                            enable: false,
                            run: RunMode::Edit,
                            result: ResultMode::Ask,
                            style: ToolCall {
                                inline_results: InlineResults::Off,
                                results_file_link: LinkStyle::Full,
                            },
                        }),
                    ],
                    input: "specific",
                    expected: Tool {
                        enable: false,
                        run: RunMode::Edit,
                        result: ResultMode::Ask,
                        style: ToolCall {
                            inline_results: InlineResults::Off,
                            results_file_link: LinkStyle::Full,
                        },
                    },
                },
            ),
            ("exact match with defaults should use defaults", TestCase {
                tools: vec![
                    ("*", Tool {
                        enable: false,
                        run: RunMode::Edit,
                        result: ResultMode::Ask,
                        style: ToolCall {
                            inline_results: InlineResults::Off,
                            results_file_link: LinkStyle::Off,
                        },
                    }),
                    ("specific", Tool {
                        enable: false,
                        run: RunMode::Edit,
                        result: ResultMode::Ask,
                        style: ToolCall {
                            inline_results: InlineResults::Off,
                            results_file_link: LinkStyle::Off,
                        },
                    }),
                ],
                input: "specific",
                expected: Tool {
                    enable: false,
                    run: RunMode::Edit,
                    result: ResultMode::Ask,
                    style: ToolCall {
                        inline_results: InlineResults::Off,
                        results_file_link: LinkStyle::Off,
                    },
                },
            }),
        ];

        for (name, test) in cases {
            let server = Server {
                enable: true,
                binary_checksum: None,
                tools: test
                    .tools
                    .into_iter()
                    .map(|(k, v)| (ToolId::new(k), v))
                    .collect::<ConfigMap<_, _>>(),
            };

            let received = server.get_tool_with_defaults(test.input);
            assert_eq!(received, test.expected, "test case: {name}");
        }
    }
}
