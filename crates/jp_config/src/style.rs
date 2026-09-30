//! Style configuration for output formatting.

pub mod code;
pub mod inline_code;
pub mod lock_wait;
pub mod markdown;
pub mod mcp_startup;
pub mod reasoning;
pub mod stderr_rows;
pub mod streaming;
pub mod tool_call;
pub mod typewriter;

use std::fmt;

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, path},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    style::{
        code::{CodeConfig, PartialCodeConfig},
        inline_code::{InlineCodeConfig, PartialInlineCodeConfig},
        lock_wait::{LockWaitConfig, PartialLockWaitConfig},
        markdown::{MarkdownConfig, PartialMarkdownConfig},
        mcp_startup::{McpStartupConfig, PartialMcpStartupConfig},
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
    /// How escape sequences in conversation content are shown.
    ///
    /// Defaults to `strip`.
    ///
    /// Your messages, the assistant's replies, tool results, and conversation
    /// titles can contain terminal escape sequences and control characters:
    /// from a pasted log, from colored command output, or written to move the
    /// cursor, clear the screen, or change the window title.
    ///
    /// - `strip`: Remove everything that does more than style text.
    ///   Your messages and tool results keep their colors, bold, and other
    ///   styling that does not hide text.
    ///   The assistant's replies, conversation titles, and search results from
    ///   `jp conversation grep` lose theirs; the assistant's markdown is still
    ///   rendered.
    /// - `visualize`: Like `strip`, and show a `␛` where something was
    ///   removed.
    /// - `off`: Show content exactly as written.
    ///
    /// Stored conversations keep the original text, and the assistant always
    /// receives it.
    ///
    /// Some protections apply whatever this is set to: window titles and links
    /// never contain control characters, tool questions are shown as plain
    /// text, styling left open by a message or tool result ends with it, and
    /// plain-text and JSON output never contain escape sequences.
    #[setting(default)]
    pub sanitize: Sanitization,

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

    /// MCP server startup indicator.
    ///
    /// Configures the timer shown while waiting for MCP servers that are still
    /// starting when a query needs them.
    #[setting(nested)]
    pub mcp_startup: McpStartupConfig,

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

    /// Lock-wait progress indicator.
    ///
    /// Configures the timer shown while waiting for a conversation lock held by
    /// another session to be released.
    #[setting(nested)]
    pub lock_wait: LockWaitConfig,

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
            "sanitize" => self.sanitize = kv.try_some_from_str()?,
            _ if kv.p("code") => self.code.assign(kv)?,
            _ if kv.p("inline_code") => self.inline_code.assign(kv)?,
            _ if kv.p("markdown") => self.markdown.assign(kv)?,
            _ if kv.p("mcp_startup") => self.mcp_startup.assign(kv)?,
            _ if kv.p("reasoning") => self.reasoning.assign(kv)?,
            _ if kv.p("lock_wait") => self.lock_wait.assign(kv)?,
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
            sanitize: delta_opt(self.sanitize.as_ref(), next.sanitize),
            code: self.code.delta(next.code),
            inline_code: self.inline_code.delta(next.inline_code),
            markdown: self.markdown.delta(next.markdown),
            mcp_startup: self.mcp_startup.delta(next.mcp_startup),
            reasoning: self.reasoning.delta(next.reasoning),
            lock_wait: self.lock_wait.delta(next.lock_wait),
            streaming: self.streaming.delta(next.streaming),
            tool_call: self.tool_call.delta(next.tool_call),
            typewriter: self.typewriter.delta(next.typewriter),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            sanitize: delta_opt(self.sanitize.as_ref(), next.sanitize),
            code: self.code.delta(next.code),
            inline_code: self.inline_code.delta_with_unsets(
                next.inline_code,
                &path(prefix, "inline_code"),
                unsets,
            ),
            markdown: self.markdown.delta(next.markdown),
            mcp_startup: self.mcp_startup.delta(next.mcp_startup),
            reasoning: self.reasoning.delta_with_unsets(
                next.reasoning,
                &path(prefix, "reasoning"),
                unsets,
            ),
            lock_wait: self.lock_wait.delta(next.lock_wait),
            streaming: self.streaming.delta(next.streaming),
            tool_call: self.tool_call.delta(next.tool_call),
            typewriter: self.typewriter.delta(next.typewriter),
        }
    }
}

impl FillDefaults for PartialStyleConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            sanitize: self.sanitize.or(defaults.sanitize),
            code: self.code.fill_from(defaults.code),
            inline_code: self.inline_code.fill_from(defaults.inline_code),
            markdown: self.markdown.fill_from(defaults.markdown),
            mcp_startup: self.mcp_startup.fill_from(defaults.mcp_startup),
            reasoning: self.reasoning.fill_from(defaults.reasoning),
            lock_wait: self.lock_wait.fill_from(defaults.lock_wait),
            streaming: self.streaming.fill_from(defaults.streaming),
            tool_call: self.tool_call.fill_from(defaults.tool_call),
            typewriter: self.typewriter.fill_from(defaults.typewriter),
        }
    }
}

impl ToPartial for StyleConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            sanitize: partial_opt(&self.sanitize, defaults.sanitize),
            code: self.code.to_partial(),
            inline_code: self.inline_code.to_partial(),
            markdown: self.markdown.to_partial(),
            mcp_startup: self.mcp_startup.to_partial(),
            reasoning: self.reasoning.to_partial(),
            lock_wait: self.lock_wait.to_partial(),
            streaming: self.streaming.to_partial(),
            tool_call: self.tool_call.to_partial(),
            typewriter: self.typewriter.to_partial(),
        }
    }
}

/// How escape sequences in conversation content are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ConfigEnum)]
#[config(rename_all = "snake_case", serde_as_string)]
pub enum Sanitization {
    /// Remove everything that does more than style text.
    #[default]
    Strip,

    /// Like `strip`, and show a `␛` where something was removed.
    Visualize,

    /// Show content exactly as written.
    Off,
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
