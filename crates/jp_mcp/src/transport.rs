use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Transport types for MCP server communication.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type")]
pub enum Transport {
    Stdio(Stdio),
}

impl From<Stdio> for Transport {
    fn from(value: Stdio) -> Self {
        Transport::Stdio(value)
    }
}

/// Standard input/output transport.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Stdio {
    pub command: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_variables: Vec<String>,
}

impl Stdio {
    pub fn cmd(cmd: impl Into<PathBuf>) -> Self {
        Self {
            command: cmd.into(),
            args: vec![],
            environment_variables: vec![],
        }
    }
}
