//! MCP provider configurations.

use std::path::PathBuf;

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_opt_vec},
    partial::{ToPartial, partial_opt, partial_opt_config},
};

/// MCP provider configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case", serde(tag = "type"))]
pub enum McpProviderConfig {
    /// Standard input/output transport.
    #[setting(nested)]
    Stdio(StdioConfig),
}

impl AssignKeyValue for PartialMcpProviderConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match self {
            Self::Stdio(config) => config.assign(kv),
        }
    }
}

impl PartialConfigDelta for PartialMcpProviderConfig {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Stdio(prev), Self::Stdio(next)) => Self::Stdio(PartialStdioConfig {
                command: delta_opt(prev.command.as_ref(), next.command),
                arguments: delta_opt_vec(prev.arguments.as_ref(), next.arguments),
                variables: delta_opt_vec(prev.variables.as_ref(), next.variables),
                checksum: delta_opt_partial(prev.checksum.as_ref(), next.checksum),
                optional: delta_opt(prev.optional.as_ref(), next.optional),
            }),
        }
    }
}

impl McpProviderConfig {
    /// Whether a failed start of this server should be tolerated.
    ///
    /// Optional servers are logged at `warn` and skipped; tools backed by
    /// them are filtered out of the resolved tool list before the LLM ever
    /// sees them. Required servers (the default) abort the command on any
    /// startup failure.
    #[must_use]
    pub const fn optional(&self) -> bool {
        match self {
            Self::Stdio(config) => config.optional,
        }
    }
}

impl ToPartial for McpProviderConfig {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Stdio(config) => Self::Partial::Stdio(config.to_partial()),
        }
    }
}

/// Standard input/output transport.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct StdioConfig {
    /// The command to run.
    pub command: PathBuf,

    /// The arguments to pass to the command.
    #[setting(default, merge = schematic::merge::append_vec)]
    pub arguments: Vec<String>,

    /// The environment variables to expose to the command.
    ///
    /// By default, the command inherits the environment of the parent process.
    /// You can use this to add additional environment variables, or override
    /// existing ones.
    #[setting(default, merge = schematic::merge::append_vec)]
    pub variables: Vec<String>,

    /// The binary checksum for the binary.
    #[setting(nested)]
    pub checksum: Option<ChecksumConfig>,

    /// Whether this MCP server is optional.
    ///
    /// When `false` (the default), a startup failure aborts the command — the
    /// same loud failure as before this field existed.
    ///
    /// When `true`, a startup failure is logged at `warn` and the server is
    /// skipped. Tools that depend on this server are filtered out before they
    /// are presented to the LLM, so the assistant never sees a tool whose
    /// backing server failed to start.
    #[setting(default)]
    pub optional: bool,
}

impl AssignKeyValue for PartialStdioConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "command" => self.command = kv.try_some_from_str()?,
            _ if kv.p("args") => kv.try_some_vec_of_strings(&mut self.arguments)?,
            _ if kv.p("env") => kv.try_some_vec_of_strings(&mut self.variables)?,
            _ if kv.p("binary_checksum") => self.checksum.assign(kv)?,
            "optional" => self.optional = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl ToPartial for StdioConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        PartialStdioConfig {
            command: partial_opt(&self.command, defaults.command),
            arguments: partial_opt(&self.arguments, defaults.arguments),
            variables: partial_opt(&self.variables, defaults.variables),
            checksum: partial_opt_config(self.checksum.as_ref(), defaults.checksum),
            optional: partial_opt(&self.optional, defaults.optional),
        }
    }
}

/// The checksum for the MCP server binary.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ChecksumConfig {
    /// The algorithm to use for the checksum.
    #[setting(default)]
    pub algorithm: AlgorithmConfig,

    /// The checksum value.
    pub value: String,
}

impl AssignKeyValue for PartialChecksumConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "algorithm" => self.algorithm = kv.try_some_from_str()?,
            "value" => self.value = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialChecksumConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            algorithm: delta_opt(self.algorithm.as_ref(), next.algorithm),
            value: delta_opt(self.value.as_ref(), next.value),
        }
    }
}

impl ToPartial for ChecksumConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            algorithm: partial_opt(&self.algorithm, defaults.algorithm),
            value: partial_opt(&self.value, defaults.value),
        }
    }
}

/// The algorithm to use for the checksum.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
pub enum AlgorithmConfig {
    /// SHA-256 checksum.
    #[default]
    #[serde(rename = "sha256")]
    Sha256,

    /// SHA-1 checksum.
    #[serde(rename = "sha1")]
    Sha1,
}

#[cfg(test)]
#[path = "mcp_tests.rs"]
mod tests;
