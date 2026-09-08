//! Provider configuration.

pub mod llm;
pub mod mcp;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_mergeable_map, path},
    fill::{FillDefaults, fill_map},
    internal::merge::map_with_strategy,
    partial::ToPartial,
    providers::{
        llm::{LlmProviderConfig, PartialLlmProviderConfig},
        mcp::McpProviderConfig,
    },
    types::map::{MergeableMap, map_to_partial_per_key},
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
    /// Configuration for Model Context Protocol (MCP) servers.
    /// The key is the server ID.
    ///
    /// ```toml
    /// [providers.mcp.bookworm]
    /// type = "stdio"
    /// command = "just"
    /// arguments = ["serve-bookworm"]
    /// ```
    ///
    /// Entries merge by key, so a server added to a later layer joins the ones
    /// an earlier layer configured rather than replacing them.
    /// Declare the map as `{ value = { … }, strategy = "replace" }` to drop
    /// them instead.
    #[setting(nested, merge = map_with_strategy)]
    pub mcp: MergeableMap<McpProviderConfig>,
}

impl AssignKeyValue for PartialProviderConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("llm") => self.llm.assign(kv)?,
            _ if kv.p("mcp") => kv.assign_to_entry(&mut self.mcp)?,
            // _ if kv.p("tts") => self.tts.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialProviderConfig {
    // Not written in terms of `delta_with_unsets`: that method reports a value
    // whose correctness depends on the field being cleared first, so a caller
    // that drops the paths would merge a list onto the one already there.
    fn delta(&self, next: Self) -> Self {
        Self {
            llm: self.llm.delta(next.llm),
            mcp: delta_mergeable_map(&self.mcp, next.mcp),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            llm: self
                .llm
                .delta_with_unsets(next.llm, &path(prefix, "llm"), unsets),
            // The map states its own strategy, so a removed server travels in
            // the value as a `replace` and needs no path reported.
            mcp: delta_mergeable_map(&self.mcp, next.mcp),
        }
    }
}

impl FillDefaults for PartialProviderConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            llm: self.llm.fill_from(defaults.llm),
            // Key by key, so a server only the defaults declare is added
            // while one this layer already has keeps its own value. A map
            // that states a strategy is left alone: its owner said how it
            // combines, and filling gaps into it would answer differently.
            mcp: match self.mcp {
                merged @ MergeableMap::Merged(_) => merged,
                MergeableMap::Map(entries) => fill_map(entries, defaults.mcp.into_map()).into(),
            },
        }
    }
}

impl ToPartial for ProviderConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            llm: self.llm.to_partial(),
            // Per key rather than `replace`: a server the workspace config
            // gained after this conversation was created still reaches it.
            mcp: map_to_partial_per_key(self.mcp.iter()),
        }
    }
}
