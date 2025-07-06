use std::str::FromStr;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    serde::de_from_str,
    style::LinkStyle,
    Error,
};

/// Tool call style configuration.
///
/// This can be overridden on a per-[`Tool`] basis.
///
/// [`Tool`]: crate::mcp::server::tool::Tool
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct ToolCall {
    /// Whether to show the tool call results inline. Even if disabled, a link
    /// will be added to a file containing the tool call results.
    #[config(default = "full", deserialize_with = de_from_str)]
    pub inline_results: InlineResults,

    /// Show a link to the file containing the source code in code blocks.
    ///
    /// Can be one of: `off`, `full`, `osc8`.
    ///
    /// See: <https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda>
    #[config(default = "osc8", deserialize_with = de_from_str)]
    pub results_file_link: LinkStyle,
}

impl AssignKeyValue for <ToolCall as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "inline_results" => self.inline_results = Some(kv.try_into_string()?.parse()?),
            "results_file_link" => self.results_file_link = Some(kv.try_into_string()?.parse()?),

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}

/// Whether and how to show the tool call results inline in the terminal.
///
/// Even if disabled or truncated, a link will be added to a file containing the
/// full tool call results. Additionally, the full tool call results will be
/// sent back to the assistant, regardless of this setting.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InlineResults {
    /// Never show the tool call results inline.
    Off,

    /// Show the full tool call results inline.
    #[default]
    Full,

    /// Show the first N lines of the tool call results inline.
    Truncate { lines: usize },
}

impl FromStr for InlineResults {
    type Err = Error;

    fn from_str(style: &str) -> Result<Self> {
        match style {
            "off" => Ok(Self::Off),
            "full" => Ok(Self::Full),
            v => v
                .parse::<usize>()
                .map(|lines| Self::Truncate { lines })
                .map_err(|_| Error::InvalidConfigValueType {
                    key: style.to_string(),
                    value: style.to_string(),
                    need: vec!["off".to_owned(), "full".to_owned(), "<number>".to_owned()],
                }),
        }
    }
}
