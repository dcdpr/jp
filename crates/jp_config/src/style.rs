//! Style configuration for output formatting.

pub mod code;
pub mod inline_code;
pub mod markdown;
pub mod reasoning;
pub mod streaming;
pub mod tool_call;
pub mod typewriter;

use std::fmt;

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    partial::ToPartial,
    style::{
        code::{CodeConfig, PartialCodeConfig},
        inline_code::{InlineCodeConfig, PartialInlineCodeConfig},
        markdown::{MarkdownConfig, PartialMarkdownConfig},
        reasoning::{PartialReasoningConfig, ReasoningConfig},
        streaming::{PartialStreamingConfig, StreamingConfig},
        tool_call::{PartialToolCallConfig, ToolCallConfig},
        typewriter::{PartialTypewriterConfig, TypewriterConfig},
    },
};

/// Style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct StyleConfig {
    /// Fenced code block style.
    ///
    /// Configures how code blocks in the assistant's response are rendered.
    #[setting(nested)]
    pub code: CodeConfig,

    /// Inline code span style.
    ///
    /// Configures how inline code (`` `like this` ``) is rendered.
    #[setting(nested)]
    pub inline_code: InlineCodeConfig,

    /// Markdown rendering style.
    ///
    /// Configures how markdown content is rendered in the terminal.
    #[setting(nested)]
    pub markdown: MarkdownConfig,

    /// Reasoning content style.
    ///
    /// Configures how the assistant's reasoning process (thinking) is
    /// displayed.
    #[setting(nested)]
    pub reasoning: ReasoningConfig,

    /// Streaming response style.
    ///
    /// Configures the waiting indicator shown while the LLM is processing.
    #[setting(nested)]
    pub streaming: StreamingConfig,

    /// Tool call content style.
    ///
    /// Configures how tool calls are displayed.
    #[setting(nested)]
    pub tool_call: ToolCallConfig,

    /// Typewriter style.
    ///
    /// Configures the typing animation effect.
    #[setting(nested)]
    pub typewriter: TypewriterConfig,
}

impl AssignKeyValue for PartialStyleConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("code") => self.code.assign(kv)?,
            _ if kv.p("inline_code") => self.inline_code.assign(kv)?,
            _ if kv.p("markdown") => self.markdown.assign(kv)?,
            _ if kv.p("reasoning") => self.reasoning.assign(kv)?,
            _ if kv.p("streaming") => self.streaming.assign(kv)?,
            _ if kv.p("tool_call") => self.tool_call.assign(kv)?,
            _ if kv.p("typewriter") => self.typewriter.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialStyleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            code: self.code.delta(next.code),
            inline_code: self.inline_code.delta(next.inline_code),
            markdown: self.markdown.delta(next.markdown),
            reasoning: self.reasoning.delta(next.reasoning),
            streaming: self.streaming.delta(next.streaming),
            tool_call: self.tool_call.delta(next.tool_call),
            typewriter: self.typewriter.delta(next.typewriter),
        }
    }
}

impl ToPartial for StyleConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            code: self.code.to_partial(),
            inline_code: self.inline_code.to_partial(),
            markdown: self.markdown.to_partial(),
            reasoning: self.reasoning.to_partial(),
            streaming: self.streaming.to_partial(),
            tool_call: self.tool_call.to_partial(),
            typewriter: self.typewriter.to_partial(),
        }
    }
}

/// Formatting style for links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum LinkStyle {
    /// No link.
    Off,
    /// Unformatted link.
    Full,
    /// Link with OSC-8 escape sequences.
    #[default]
    Osc8,
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

#[cfg(test)]
#[path = "style_tests.rs"]
mod tests;
