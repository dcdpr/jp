//! Provider configuration.

pub mod llm;
pub mod mcp;

use indexmap::IndexMap;
use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::PartialConfigDelta,
    partial::ToPartial,
    providers::{
        llm::{LlmProviderConfig, PartialLlmProviderConfig},
        mcp::McpProviderConfig,
    },
    util::merge_nested_indexmap,
};

/// Provider configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ProviderConfig {
    /// LLM provider configurations.
    ///
    /// Configuration for different LLM providers (e.g. Anthropic, OpenAI,
    /// Ollama).
    #[setting(nested)]
    pub llm: LlmProviderConfig,

    /// MCP provider configurations.
    ///
    /// Configuration for Model Context Protocol (MCP) servers. The key is the
    /// server ID.
    #[setting(nested, merge = merge_nested_indexmap)]
    pub mcp: IndexMap<String, McpProviderConfig>,
}

impl AssignKeyValue for PartialProviderConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("llm") => self.llm.assign(kv)?,
            _ if kv.p("mcp") => match kv.trim_prefix_any() {
                Some(name) => self.mcp.entry(name).or_default().assign(kv)?,
                None => return missing_key(&kv),
            },
            // _ if kv.p("tts") => self.tts.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialProviderConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            llm: self.llm.delta(next.llm),
            mcp: next
                .mcp
                .into_iter()
                .filter_map(|(k, next)| {
                    let prev = self.mcp.get(&k);
                    if prev.is_some_and(|prev| prev == &next) {
                        return None;
                    }

                    let next = match prev {
                        Some(prev) => prev.delta(next),
                        None => next,
                    };

                    Some((k, next))
                })
                .collect(),
        }
    }
}

impl ToPartial for ProviderConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            llm: self.llm.to_partial(),
            mcp: self
                .mcp
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
        }
    }
}
