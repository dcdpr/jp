use std::{collections::HashMap, fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Error;

/// Identifier for an MCP tool.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct McpToolId(String);

impl McpToolId {
    // TODO: implement `FromStr` or `TryFrom`, to reject invalid IDs.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for McpToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Metadata for all MCP tools.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpToolsMetadata {
    pub templates: HashMap<String, McpToolTemplate>,
}

/// Configuration for an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct McpTool {
    #[serde(skip)]
    pub id: McpToolId,
    pub description: String,
    pub command: Vec<String>,
    pub properties: Vec<Map<String, Value>>,
}

/// Template for an MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct McpToolTemplate {
    pub command: Vec<String>,
}

macro_rules! named_unit_variant {
    ($variant:ident) => {
        pub mod $variant {
            pub fn serialize<S>(serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(stringify!($variant))
            }

            pub fn deserialize<'de, D>(deserializer: D) -> Result<(), D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct V;
                impl<'de> serde::de::Visitor<'de> for V {
                    type Value = ();

                    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str(concat!("\"", stringify!($variant), "\""))
                    }

                    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        if value == stringify!($variant) {
                            Ok(())
                        } else {
                            Err(E::invalid_value(serde::de::Unexpected::Str(value), &self))
                        }
                    }
                }

                deserializer.deserialize_str(V)
            }
        }
    };
}

mod strings {
    named_unit_variant!(auto);
    named_unit_variant!(none);
    named_unit_variant!(required);
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// Call zero, one, or multiple tools, at the discretion of the LLM.
    #[default]
    #[serde(with = "strings::auto")]
    Auto,

    /// Force the LLM not to call any tools, even if any are available.
    #[serde(with = "strings::none")]
    None,

    /// Force the LLM to call at least one tool.
    #[serde(with = "strings::required")]
    Required,

    /// Require calling the specified named tool.
    Function(String),
}

impl FromStr for ToolChoice {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "auto" => Ok(Self::Auto),
            "none" | "false" => Ok(Self::None),
            "required" | "true" => Ok(Self::Required),
            s if s.chars().all(|c| c.is_alphanumeric() || c == '_') => {
                Ok(Self::Function(s.to_owned()))
            }
            _ if s.starts_with("fn:") && s.len() > 3 => Ok(Self::Function(s[3..].to_owned())),
            _ => Err(Error::UnknownToolChoice(s.to_string())),
        }
    }
}
