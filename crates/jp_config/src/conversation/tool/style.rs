//! Display style configuration for tools.

use std::{fmt, num::ParseIntError, str::FromStr};

use schematic::{Config, ConfigEnum, Schematic};
use serde::{Deserialize, Serialize};

use crate::{
    BoxedError,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    conversation::tool::CommandConfigOrString,
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
};

/// Display style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct DisplayStyleConfig {
    /// How to display the results of the tool call.
    #[setting(default)]
    pub inline_results: InlineResults,

    /// How to display the link to the file containing the tool call results.
    #[setting(default)]
    pub results_file_link: LinkStyle,

    /// How to display the tool call parameters.
    #[setting(default)]
    pub parameters: ParametersStyle,
}

impl AssignKeyValue for PartialDisplayStyleConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "inline_results" => self.inline_results = kv.try_some_from_str()?,
            "results_file_link" => self.results_file_link = kv.try_some_from_str()?,
            "parameters" => self.parameters = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialDisplayStyleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            inline_results: delta_opt(self.inline_results.as_ref(), next.inline_results),
            results_file_link: delta_opt(self.results_file_link.as_ref(), next.results_file_link),
            parameters: delta_opt(self.parameters.as_ref(), next.parameters),
        }
    }
}

impl ToPartial for DisplayStyleConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            inline_results: partial_opt(&self.inline_results, defaults.inline_results),
            results_file_link: partial_opt(&self.results_file_link, defaults.results_file_link),
            parameters: partial_opt(&self.parameters, defaults.parameters),
        }
    }
}

/// Whether and how to show the tool call results inline in the terminal.
///
/// Even if disabled or truncated, a link will be added to a file containing the
/// full tool call results. Additionally, the full tool call results will be
/// sent back to the assistant, regardless of this setting.
#[derive(Debug, Clone, PartialEq, Serialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum InlineResults {
    /// Never show the tool call results inline.
    Off,

    /// Show the full tool call results inline.
    Full,

    /// Show the first N lines of the tool call results inline.
    #[variant(fallback)]
    Truncate(TruncateLines),
}

impl Default for InlineResults {
    fn default() -> Self {
        Self::Truncate(TruncateLines::default())
    }
}

impl<'de> Deserialize<'de> for InlineResults {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct InlineResultsVisitor;

        impl<'de> serde::de::Visitor<'de> for InlineResultsVisitor {
            type Value = InlineResults;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a boolean, a string (\"off\", \"full\"), or a number for truncation",
                )
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v {
                    Ok(InlineResults::Full)
                } else {
                    Ok(InlineResults::Off)
                }
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    "off" => Ok(InlineResults::Off),
                    "full" => Ok(InlineResults::Full),
                    s => s
                        .parse::<usize>()
                        .map(|lines| InlineResults::Truncate(TruncateLines { lines }))
                        .map_err(|_| {
                            serde::de::Error::unknown_variant(v, &["off", "full", "a number"])
                        }),
                }
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                usize::try_from(v)
                    .map(|lines| InlineResults::Truncate(TruncateLines { lines }))
                    .map_err(|_| {
                        serde::de::Error::invalid_value(
                            serde::de::Unexpected::Unsigned(v),
                            &"a number",
                        )
                    })
            }

            fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                // Reuse the derived deserializer for the complex case (Truncate object)
                #[derive(Deserialize)]
                #[serde(rename_all = "snake_case")]
                enum Helper {
                    Off,
                    Full,
                    Truncate(TruncateLines),
                }

                let helper =
                    Helper::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                match helper {
                    Helper::Off => Ok(InlineResults::Off),
                    Helper::Full => Ok(InlineResults::Full),
                    Helper::Truncate(t) => Ok(InlineResults::Truncate(t)),
                }
            }
        }

        deserializer.deserialize_any(InlineResultsVisitor)
    }
}

/// Truncate the tool call results to the first N lines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TruncateLines {
    /// The number of lines to show.
    pub lines: usize,
}

impl Default for TruncateLines {
    fn default() -> Self {
        Self { lines: 10 }
    }
}

impl TryFrom<&str> for TruncateLines {
    type Error = ParseIntError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse().map(|lines| Self { lines })
    }
}

impl fmt::Display for TruncateLines {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.lines)
    }
}

/// How to display the link to the file containing the tool call results.
#[derive(Debug, Clone, PartialEq, Default, Serialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum LinkStyle {
    /// Full (raw) link.
    #[default]
    Full,

    /// Clickable link using the `osc8` escape sequence.
    Osc8,

    /// No link.
    Off,
}

impl<'de> Deserialize<'de> for LinkStyle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct LinkStyleVisitor;

        impl serde::de::Visitor<'_> for LinkStyleVisitor {
            type Value = LinkStyle;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a boolean or a string (\"off\", \"full\", \"osc8\")")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v {
                    Ok(LinkStyle::Full)
                } else {
                    Ok(LinkStyle::Off)
                }
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match v {
                    "off" => Ok(LinkStyle::Off),
                    "full" => Ok(LinkStyle::Full),
                    "osc8" => Ok(LinkStyle::Osc8),
                    _ => Err(serde::de::Error::unknown_variant(v, &[
                        "off", "full", "osc8",
                    ])),
                }
            }
        }

        deserializer.deserialize_any(LinkStyleVisitor)
    }
}

/// Define the name to serialize and deserialize for a unit variant.
mod strings {
    use crate::named_unit_variant;

    named_unit_variant!(json);
    named_unit_variant!(function_call);
    named_unit_variant!(off);
}

/// How to display the link to the file containing the tool call results.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, Schematic)]
#[serde(untagged)]
pub enum ParametersStyle {
    /// A JSON representation of the tool call parameters.
    #[default]
    #[serde(with = "strings::json")]
    #[schema(literal = "json")]
    Json,

    /// Function call syntax.
    ///
    /// e.g. `tool_name(arg1 = "value", arg2 = 123)`.
    #[serde(with = "strings::function_call")]
    #[schema(literal = "function_call")]
    FunctionCall,

    /// No parameters are displayed.
    #[serde(with = "strings::off")]
    #[schema(literal = "off")]
    Off,

    /// A custom command to format how the tool call parameters are displayed in
    /// the terminal.
    ///
    /// The command is provided with the json representation of the parameters,
    /// and is expected to return a string that is shown verbatim in the
    /// terminal.
    Custom(CommandConfigOrString),
}

impl FromStr for ParametersStyle {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "off" => Self::Off,
            "json" => Self::Json,
            "function_call" => Self::FunctionCall,
            _ => Self::Custom(CommandConfigOrString::String(s.to_owned())),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use super::*;

    #[test]
    fn test_link_style_deserialization() {
        assert_eq!(from_str::<LinkStyle>("false").unwrap(), LinkStyle::Off);
        assert_eq!(from_str::<LinkStyle>("true").unwrap(), LinkStyle::Full);
        assert_eq!(from_str::<LinkStyle>("\"off\"").unwrap(), LinkStyle::Off);
        assert_eq!(from_str::<LinkStyle>("\"full\"").unwrap(), LinkStyle::Full);
        assert_eq!(from_str::<LinkStyle>("\"osc8\"").unwrap(), LinkStyle::Osc8);
    }

    #[test]
    fn test_inline_results_deserialization() {
        assert_eq!(
            from_str::<InlineResults>("false").unwrap(),
            InlineResults::Off
        );
        assert_eq!(
            from_str::<InlineResults>("true").unwrap(),
            InlineResults::Full
        );
        assert_eq!(
            from_str::<InlineResults>("\"off\"").unwrap(),
            InlineResults::Off
        );
        assert_eq!(
            from_str::<InlineResults>("\"full\"").unwrap(),
            InlineResults::Full
        );
        assert_eq!(
            from_str::<InlineResults>("10").unwrap(),
            InlineResults::Truncate(TruncateLines { lines: 10 })
        );
        assert_eq!(
            from_str::<InlineResults>("\"25\"").unwrap(),
            InlineResults::Truncate(TruncateLines { lines: 25 })
        );
        assert_eq!(
            from_str::<InlineResults>(r#"{"truncate": {"lines": 5}}"#).unwrap(),
            InlineResults::Truncate(TruncateLines { lines: 5 })
        );
    }
}
