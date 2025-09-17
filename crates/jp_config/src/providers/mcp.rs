//! MCP provider configurations.

use std::path::PathBuf;

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment};

/// MCP provider configuration.
#[derive(Debug, Clone, Config)]
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

/// Standard input/output transport.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct StdioConfig {
    /// The command to run.
    pub command: PathBuf,

    /// The arguments to pass to the command.
    #[setting(default, merge = schematic::merge::append_vec)]
    pub args: Vec<String>,

    /// The environment variables to expose to the command.
    #[setting(default, rename = "env", merge = schematic::merge::append_vec)]
    pub environment_variables: Vec<String>,

    /// The binary checksum for the binary.
    #[setting(nested)]
    pub binary_checksum: Option<ChecksumConfig>,
}

impl AssignKeyValue for PartialStdioConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "command" => self.command = kv.try_some_from_str()?,
            _ if kv.p("args") => kv.try_some_vec_of_strings(&mut self.args)?,
            _ if kv.p("env") => kv.try_some_vec_of_strings(&mut self.environment_variables)?,
            _ if kv.p("binary_checksum") => self.binary_checksum.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

/// The checksum for the MCP server binary.
#[derive(Debug, Clone, Config)]
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
            "" => *self = kv.try_object()?,
            "algorithm" => self.algorithm = kv.try_some_from_str()?,
            "value" => self.value = kv.try_some_string()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

/// The algorithm to use for the checksum.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[config]
pub enum AlgorithmConfig {
    /// SHA-256 checksum.
    #[default]
    #[serde(rename = "sha256")]
    Sha256,

    /// SHA-1 checksum.
    #[serde(rename = "sha1")]
    Sha1,
}
