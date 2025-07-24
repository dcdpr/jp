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
    mcp::server::ToolId,
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

impl Mcp {
    #[must_use]
    pub fn get_server(&self, id: &ServerId) -> Server {
        self.servers
            .get(id)
            .cloned()
            .or_else(|| self.servers.get(&ServerId::new("*")).cloned())
            .unwrap_or_default()
    }
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

impl ServerId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

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
            servers: self.merge_servers_with_inheritance(&fallback),
        }
    }

    fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    fn is_complete(&self) -> bool {
        self.servers.is_complete()
    }
}

impl McpPartial {
    /// Get the server with the given ID, or an empty server if it does not
    /// exist.
    #[must_use]
    fn get_server_or_empty(&self, server_id: &ServerId) -> ServerPartial {
        self.servers
            .get(server_id)
            .cloned()
            .unwrap_or(ServerPartial::empty())
    }

    /// Merge the servers of this configuration with the ones of the given
    /// configuration, using nested inheritance.
    ///
    /// Servers inherit their configuration from the global (`*`) server, if the
    /// server itself does not have a configuration for a given field.
    ///
    /// Tools have a more complex inheritance scheme:
    ///
    /// ```markdown,ignore
    /// 1. server  .servers.my_server.tools.my_tool
    /// 2. fallback.servers.my_server.tools.my_tool
    /// 3. server  .servers.my_server.tools.*
    /// 4. fallback.servers.my_server.tools.*
    /// 5. server  .servers.*        .tools.my_tool
    /// 6. fallback.servers.*        .tools.my_tool
    /// 7. server  .servers.*        .tools.*
    /// 8. fallback.servers.*        .tools.*
    /// ```
    ///
    /// Both `self` and `other` are merged together in the correct order.
    fn merge_servers_with_inheritance(
        &self,
        other: &Self,
    ) -> ConfigMapPartial<ServerId, ServerPartial> {
        // Iterate over all server IDs.
        let ids = self.servers.keys().chain(other.servers.keys());

        let mut result = ConfigMapPartial::default();
        for server_id in ids {
            let server = self.get_server_or_empty(server_id);
            let fallback = other.get_server_or_empty(server_id);

            let merged_server = if server_id.as_str() == "*" {
                server.with_fallback(fallback)
            } else {
                let global_server = self.get_server_or_empty(&ServerId::new("*"));
                let global_fallback = other.get_server_or_empty(&ServerId::new("*"));

                // Handle tools with mixed server/tool inheritance
                let tools = server
                    .tools
                    .keys()
                    .chain(fallback.tools.keys())
                    .chain(global_server.tools.keys())
                    .chain(global_fallback.tools.keys())
                    .map(|id| {
                        let server_tool = server.get_tool_or_empty(id);
                        let server_tool_global = server.get_tool_or_empty(&ToolId::new("*"));
                        let global_server_tool = global_server.get_tool_or_empty(id);
                        let global_server_tool_global =
                            global_server.get_tool_or_empty(&ToolId::new("*"));

                        let fallback_tool = fallback.get_tool_or_empty(id);
                        let fallback_tool_global = fallback.get_tool_or_empty(&ToolId::new("*"));
                        let global_fallback_tool = global_fallback.get_tool_or_empty(id);
                        let global_fallback_tool_global =
                            global_fallback.get_tool_or_empty(&ToolId::new("*"));

                        let tool = server_tool
                            .with_fallback(fallback_tool)
                            .with_fallback(server_tool_global)
                            .with_fallback(fallback_tool_global)
                            .with_fallback(global_server_tool)
                            .with_fallback(global_fallback_tool)
                            .with_fallback(global_server_tool_global)
                            .with_fallback(global_fallback_tool_global);

                        (id.clone(), tool)
                    })
                    .collect();

                // Merge server-level fields (enable, binary_checksum) with simple inheritance
                let mut server = server
                    .with_fallback(fallback)
                    .with_fallback(global_server)
                    .with_fallback(global_fallback);

                server.tools = tools;
                server
            };

            result.insert(server_id.clone(), merged_server);
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::mcp::{
        server::{
            checksum::{Algorithm, Checksum},
            tool::{ResultMode, RunMode, ToolPartial},
        },
        tool_call::confique_partial_tool_call::PartialToolCall,
    };

    #[test]
    #[expect(clippy::too_many_lines, clippy::needless_raw_string_hashes)]
    fn test_server_partial_with_fallback() {
        struct TestCase {
            config: &'static str,
            fallback: &'static str,
            expected: &'static str,
        }

        let cases = vec![
            ("direct", TestCase {
                config: indoc! {r#"
                    [servers."*".tools.github_issues]
                    run = "always"
                "#},
                fallback: indoc! {r#"
                    [servers.embedded.tools.github_issues]
                    result = "ask"
                "#},
                expected: indoc! {r#"
                    [servers.embedded.tools.github_issues]
                    run = "always"
                    result = "ask"
                "#},
            }),
            ("global_tool_inheritance", TestCase {
                config: indoc! {r#"
                    [servers."*".tools."*"]
                    enable = false
                    run = "always"
                    result = "ask"
                "#},
                fallback: indoc! {r#"
                    [servers.embedded.tools.github_issues]
                    enable = true
                "#},
                expected: indoc! {r#"
                    [servers.embedded.tools.github_issues]
                    enable = true
                    run = "always"
                    result = "ask"
                "#},
            }),
            ("server_level_fallback", TestCase {
                config: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    run = "edit"
                "#},
                fallback: indoc! {r#"
                    [servers.embedded.binary_checksum]
                    value = "abc123"

                    [servers.embedded.tools.github_issues]
                    result = "always"
                "#},
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    run = "edit"
                    result = "always"

                    [servers.embedded.binary_checksum]
                    value = "abc123"
                "#},
            }),
            ("complex_2d_inheritance", TestCase {
                config: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    run = "always"
                    result = "always"

                    [servers."*".tools.github_issues]
                    enable = false

                    [servers.embedded.tools."*"]
                    run = "edit"
                "#},
                fallback: indoc! {r#"
                    [servers.embedded.binary_checksum]
                    value = "fallback123"

                    [servers.embedded.tools.github_issues]
                    result = "ask"
                "#},
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    enable = false
                    run = "edit"
                    result = "ask"

                    [servers.embedded.binary_checksum]
                    value = "fallback123"
                "#},
            }),
            ("tool_style_inheritance", TestCase {
                config: indoc! {r#"
                    [servers."*".tools."*"]
                    enable = true

                    [servers."*".tools."*".style]
                    inline_results = "off"
                    results_file_link = "off"
                "#},
                fallback: indoc! {r#"
                    [servers.embedded.tools.github_issues.style]
                    inline_results = "off"
                    results_file_link = "full"
                "#},
                expected: indoc! {r#"
                    [servers.embedded.tools.github_issues]
                    enable = true

                    [servers.embedded.tools.github_issues.style]
                    inline_results = "off"
                    results_file_link = "full"
                "#},
            }),
            ("mixed_specific_and_global", TestCase {
                config: indoc! {r#"
                    [servers."*".tools."*"]
                    enable = false
                    run = "always"

                    [servers.embedded.tools."*"]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    run = "edit"
                "#},
                fallback: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".binary_checksum]
                    value = "global_fallback"

                    [servers.another.tools.github_issues]
                    result = "ask"
                "#},
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    enable = true
                    run = "edit"

                    [servers.embedded.binary_checksum]
                    value = "global_fallback"

                    [servers.another]
                    enable = true

                    [servers.another.tools.github_issues]
                    enable = false
                    run = "always"
                    result = "ask"

                    [servers.another.binary_checksum]
                    value = "global_fallback"
                "#},
            }),
            ("deep_tool_config_override", TestCase {
                config: indoc! {r#"
                    [servers."*".tools."*"]
                    run = "always"

                    [servers."*".tools."*".style]
                    inline_results = "off"

                    [servers.embedded.tools.github_issues.style]
                    inline_results = "50"
                    results_file_link = "osc8"
                "#},
                fallback: indoc! {r#"
                    [servers.embedded.tools."*"]
                    result = "edit"

                    [servers.embedded.tools."*".style]
                    results_file_link = "off"
                "#},
                expected: indoc! {r#"
                    [servers.embedded.tools.github_issues]
                    run = "always"
                    result = "edit"

                    [servers.embedded.tools.github_issues.style]
                    inline_results = "50"
                    results_file_link = "osc8"
                "#},
            }),
            ("empty_config_with_fallback", TestCase {
                config: "",
                fallback: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    run = "always"
                    result = "always"

                    [servers.embedded.tools.github_issues]
                    enable = false
                "#},
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    enable = false
                    run = "always"
                    result = "always"
                "#},
            }),
            ("fallback_empty", TestCase {
                config: indoc! {r#"
                    [servers.embedded]
                    enable = false

                    [servers.embedded.tools.github_issues]
                    run = "edit"
                    result = "ask"
                "#},
                fallback: "",
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = false

                    [servers.embedded.tools.github_issues]
                    run = "edit"
                    result = "ask"
                "#},
            }),
            ("global server, global tool only", TestCase {
                config: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    enable = false
                    run = "always"
                    result = "ask"
                    style.inline_results = "off"
                    style.results_file_link = "off"

                    [servers.embedded.tools.github_issues]
                "#},
                fallback: "",
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    enable = false
                    run = "always"
                    result = "ask"

                    [servers.embedded.tools.github_issues.style]
                    inline_results = "off"
                    results_file_link = "off"
                "#},
            }),
            ("global server, specific tool overrides enable", TestCase {
                config: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    enable = false
                    run = "always"
                    result = "ask"
                    style.inline_results = "off"
                    style.results_file_link = "off"

                    [servers."*".tools.github_issues]
                    enable = true
                    style.inline_results = "full"
                    style.results_file_link = "osc8"

                    [servers.embedded.tools.github_issues]
                "#},
                fallback: "",
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    enable = true
                    run = "always"
                    result = "ask"
                "#},
            }),
            ("specific server, global tool overrides run", TestCase {
                config: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    enable = false
                    run = "always"
                    result = "ask"
                    style.inline_results = "off"
                    style.results_file_link = "off"

                    [servers.embedded.tools."*"]
                    run = "edit"
                    result = "edit"

                    [servers.embedded.tools.github_issues]
                "#},
                fallback: "",
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    enable = false
                    run = "edit"
                    result = "edit"

                    [servers.embedded.tools.github_issues.style]
                    inline_results = "off"
                    results_file_link = "off"
                "#},
            }),
            ("complex inheritance chain", TestCase {
                config: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    enable = false
                    run = "always"
                    result = "always"
                    style.inline_results = "full"
                    style.results_file_link = "off"

                    [servers."*".tools.github_issues]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    result = "edit"
                    style.inline_results = "10"

                    [servers.embedded.tools."*"]
                    run = "edit"
                    result = "ask"
                "#},
                fallback: "",
                expected: indoc! {r#"
                    [servers.embedded]
                    enable = true

                    [servers.embedded.tools.github_issues]
                    enable = true
                    run = "edit"
                    result = "edit"

                    [servers.embedded.tools.github_issues.style]
                    inline_results = "10"
                    results_file_link = "off"
                "#},
            }),
            (
                "complex inheritance with Some None distinctions",
                TestCase {
                    config: indoc! {r#"
                    [servers."*"]
                    enable = true

                    [servers."*".tools."*"]
                    enable = false
                    run = "always"
                    result = "always"
                    style.inline_results = "off"
                    style.results_file_link = "off"

                    [servers.embedded.tools.github_issues]
                    enable = true
                "#},
                    fallback: "",
                    expected: indoc! {r#"
                        [servers.embedded]
                        enable = true

                        [servers.embedded.tools.github_issues]
                        enable = true
                        run = "always"
                        result = "always"

                        [servers.embedded.tools.github_issues.style]
                        inline_results = "off"
                        results_file_link = "off"
                "#},
                },
            ),
        ];

        for (name, test) in cases {
            let config = toml::from_str::<McpPartial>(test.config).unwrap();
            let fallback = toml::from_str::<McpPartial>(test.fallback).unwrap();

            let mut merged = config.with_fallback(fallback);

            // Filter out the "*" servers and tools, as we're not testing them.
            merged.servers = merged
                .servers
                .into_iter()
                .filter_map(|(k, mut v)| {
                    if k.as_str() == "*" {
                        return None;
                    }

                    v.tools = v
                        .tools
                        .into_iter()
                        .filter_map(|(k, v)| {
                            if k.as_str() == "*" {
                                return None;
                            }

                            Some((k, v))
                        })
                        .collect();

                    Some((k, v))
                })
                .collect();

            let actual = toml::to_string_pretty(&merged).unwrap();

            assert_eq!(test.expected.to_owned(), actual, "test case: {name}");
        }
    }

    #[test]
    fn test_server_partial_with_fallback_merges_multiple_wildcards() {
        let partial = McpPartial {
            servers: ConfigMapPartial::from_iter([
                (ServerId("*".to_string()), ServerPartial {
                    tools: ConfigMapPartial::from_iter([(ToolId::new("*"), ToolPartial {
                        run: Some(RunMode::Ask),
                        enable: None,
                        result: None,
                        style: PartialToolCall::empty(),
                    })]),
                    ..Default::default()
                }),
                (ServerId("test".to_string()), ServerPartial {
                    tools: ConfigMapPartial::from_iter([(ToolId::new("*"), ToolPartial {
                        result: Some(ResultMode::Edit),
                        enable: None,
                        run: None,
                        style: PartialToolCall::empty(),
                    })]),
                    ..Default::default()
                }),
            ]),
        };

        let partial = partial.with_fallback(McpPartial::empty());
        let tool = partial
            .servers
            .get(&ServerId::new("test"))
            .unwrap()
            .tools
            .get(&ToolId::new("*"))
            .unwrap();

        assert_eq!(tool.result, Some(ResultMode::Edit));
        assert_eq!(tool.run, Some(RunMode::Ask));
    }

    #[test]
    fn test_mcp_get_server() {
        let config = Mcp {
            servers: ConfigMap::from_iter([
                (ServerId::new("test"), Server {
                    enable: false,
                    ..Default::default()
                }),
                (ServerId::new("*"), Server {
                    binary_checksum: Some(Checksum {
                        algorithm: Algorithm::Sha256,
                        value: "1234567890".to_string(),
                    }),
                    ..Default::default()
                }),
            ]),
        };

        let server1 = config.get_server(&ServerId::new("test"));
        assert!(!server1.enable);
        assert_eq!(server1.binary_checksum, None);

        let server2 = config.get_server(&ServerId::new("*"));
        assert!(server2.enable);
        assert_eq!(
            server2.binary_checksum,
            Some(Checksum {
                algorithm: Algorithm::Sha256,
                value: "1234567890".to_string(),
            })
        );

        let server3 = config.get_server(&ServerId::new("nonexistent"));
        assert_eq!(server2, server3);
    }
}
