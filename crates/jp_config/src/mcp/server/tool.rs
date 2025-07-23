use std::str::FromStr;

use confique::{
    meta::{Field, FieldKind, LeafKind, Meta},
    Config as Confique, Partial,
};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    mcp::tool_call::{InlineResults, ToolCall},
    serde::is_nested_default_or_empty,
    style::LinkStyle,
    Error,
};

/// MCP server tool configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    /// Whether the tool is enabled.
    pub enable: bool,

    /// How to handle running the tool call.
    pub run: RunMode,

    /// How to handle sending the results of the tool call.
    pub result: ResultMode,

    /// How to style this tool call.
    pub style: ToolCall,
}

impl Default for Tool {
    fn default() -> Self {
        Self {
            enable: true,
            run: RunMode::Ask,
            result: ResultMode::Always,
            style: ToolCall {
                inline_results: InlineResults::Full,
                results_file_link: LinkStyle::Osc8,
            },
        }
    }
}

impl Confique for Tool {
    type Partial = ToolPartial;

    const META: Meta = Meta {
        name: "tool",
        doc: &[],
        fields: &[
            Field {
                name: "enable",
                doc: &[],
                kind: FieldKind::Leaf {
                    env: None,
                    kind: LeafKind::Optional,
                },
            },
            Field {
                name: "run",
                doc: &[],
                kind: FieldKind::Leaf {
                    env: None,
                    kind: LeafKind::Optional,
                },
            },
            Field {
                name: "result",
                doc: &[],
                kind: FieldKind::Leaf {
                    env: None,
                    kind: LeafKind::Optional,
                },
            },
            Field {
                name: "style",
                doc: &[],
                kind: FieldKind::Nested {
                    meta: &ToolCall::META,
                },
            },
        ],
    };

    fn from_partial(partial: Self::Partial) -> Result<Self, confique::Error> {
        Ok(Self {
            enable: partial.enable.unwrap_or(true),
            run: partial.run.unwrap_or(RunMode::Ask),
            result: partial.result.unwrap_or(ResultMode::Always),
            style: ToolCall::from_partial(partial.style)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPartial {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResultMode>,
    #[serde(
        default = "confique::Partial::empty",
        skip_serializing_if = "is_nested_default_or_empty"
    )]
    pub style: <ToolCall as Confique>::Partial,
}

impl AssignKeyValue for ToolPartial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<(), Error> {
        let k = kv.key().as_str().to_owned();

        match k.as_str() {
            "enable" => self.enable = Some(kv.try_into_bool()?),
            "run" => self.run = Some(kv.try_into_string()?.parse()?),
            "result" => self.result = Some(kv.try_into_string()?.parse()?),
            "style" => self.style = kv.try_into_object()?,

            _ if kv.trim_prefix("style") => self.style.assign(kv)?,

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}

impl Default for ToolPartial {
    fn default() -> Self {
        Self {
            enable: Some(true),
            run: Some(RunMode::Ask),
            result: Some(ResultMode::Always),
            style: <ToolCall as Confique>::Partial::default_values(),
        }
    }
}

impl Partial for ToolPartial {
    fn empty() -> Self {
        Self {
            enable: None,
            run: None,
            result: None,
            style: <ToolCall as Confique>::Partial::empty(),
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
            run: self.run.or(fallback.run),
            result: self.result.or(fallback.result),
            style: self.style.with_fallback(fallback.style),
        }
    }

    fn is_empty(&self) -> bool {
        self.enable.is_none()
            && self.run.is_none()
            && self.result.is_none()
            && self.style.is_empty()
    }

    fn is_complete(&self) -> bool {
        self.enable.is_some()
            && self.run.is_some()
            && self.result.is_some()
            && self.style.is_complete()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    /// Ask for confirmation before running the tool.
    #[default]
    Ask,
    /// Always run the tool, without asking for confirmation.
    Always,
    /// Open an editor to edit the tool call before running it.
    Edit,
}

impl FromStr for RunMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "ask" => Ok(Self::Ask),
            "always" => Ok(Self::Always),
            "edit" => Ok(Self::Edit),
            _ => Err(Error::InvalidConfigValueType {
                key: s.to_string(),
                value: s.to_string(),
                need: vec!["always".to_string(), "ask".to_string(), "edit".to_string()],
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultMode {
    /// Always deliver the results of the tool call.
    #[default]
    Always,
    /// Ask for confirmation before delivering the results of the tool call.
    Ask,
    /// Open an editor to edit the tool call result before delivering it.
    Edit,
}

impl FromStr for ResultMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "always" => Ok(Self::Always),
            "ask" => Ok(Self::Ask),
            "edit" => Ok(Self::Edit),
            _ => Err(Error::InvalidConfigValueType {
                key: s.to_string(),
                value: s.to_string(),
                need: vec!["always".to_string(), "ask".to_string(), "edit".to_string()],
            }),
        }
    }
}
