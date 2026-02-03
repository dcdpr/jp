//! Tool configuration for conversations.

use schematic::Config;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    conversation::tool::{OneOrManyTypes, PartialOneOrManyTypes, ToolParameterConfig},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt, partial_opts},
};

/// Tool parameter configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolParameterItemConfig {
    /// The type of the parameter.
    #[setting(nested, rename = "type")]
    #[serde(rename = "type")]
    pub kind: OneOrManyTypes,

    /// The default value of the parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,

    /// Description of the parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// A list of possible values for the parameter.
    #[setting(rename = "enum", skip_serializing_if = "Option::is_none")]
    #[serde(default, rename = "enum", skip_serializing_if = "Vec::is_empty")]
    pub enumeration: Vec<Value>,
}

impl PartialConfigDelta for PartialToolParameterItemConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            kind: self.kind.delta(next.kind),
            default: delta_opt(self.default.as_ref(), next.default),
            description: delta_opt(self.description.as_ref(), next.description),
            enumeration: delta_opt(self.enumeration.as_ref(), next.enumeration),
        }
    }
}

impl ToPartial for ToolParameterItemConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            kind: self.kind.to_partial(),
            default: partial_opts(self.default.as_ref(), defaults.default),
            description: partial_opts(self.description.as_ref(), defaults.description),
            enumeration: partial_opt(&self.enumeration, defaults.enumeration),
        }
    }
}

impl From<ToolParameterItemConfig> for ToolParameterConfig {
    fn from(config: ToolParameterItemConfig) -> Self {
        Self {
            kind: config.kind,
            default: config.default,
            required: false,
            description: config.description,
            enumeration: config.enumeration,
            items: None,
        }
    }
}

impl From<ToolParameterConfig> for ToolParameterItemConfig {
    fn from(config: ToolParameterConfig) -> Self {
        Self {
            kind: config.kind,
            default: config.default,
            description: config.description,
            enumeration: config.enumeration,
        }
    }
}
