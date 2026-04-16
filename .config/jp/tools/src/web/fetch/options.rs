//! Parsing for the `web_fetch` tool's `options` map.
//!
//! See `.jp/mcp/tools/web/fetch.toml` for the user-facing schema.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use crate::Error;

/// Fetch strategy for a given URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Strategy {
    /// Try the `.md` variant first; fall back to HTML on failure.
    #[default]
    Auto,
    /// Only fetch the `.md` variant. Error if unavailable.
    Markdown,
    /// Fetch HTML directly (legacy behavior; skips the `.md` probe).
    Html,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WebFetchOptions {
    #[serde(default)]
    strategy: Strategy,

    /// Per-host strategy overrides. Keys are hostnames (exact match) or
    /// `*.suffix` wildcards.
    #[serde(default)]
    domains: HashMap<String, Strategy>,
}

impl WebFetchOptions {
    pub(super) fn parse(options: &Map<String, Value>) -> Result<Self, Error> {
        serde_json::from_value(Value::Object(options.clone()))
            .map_err(|e| format!("invalid web_fetch options: {e}").into())
    }

    /// Pick the strategy to use for the given URL.
    ///
    /// Resolution order:
    /// 1. Exact hostname match.
    /// 2. Longest matching `*.suffix` wildcard.
    /// 3. Default strategy.
    pub(super) fn pick_strategy(&self, url: &Url) -> Strategy {
        let Some(host) = url.host_str() else {
            return self.strategy;
        };
        let host = host.trim_end_matches('.').to_ascii_lowercase();

        if let Some(&s) = self.domains.get(&host) {
            return s;
        }

        self.domains
            .iter()
            .filter_map(|(pattern, strategy)| {
                let suffix = pattern.to_ascii_lowercase();
                let suffix = suffix.strip_prefix("*.")?;
                if host == suffix || host.ends_with(&format!(".{suffix}")) {
                    Some((suffix.len(), *strategy))
                } else {
                    None
                }
            })
            .max_by_key(|(len, _)| *len)
            .map_or(self.strategy, |(_, s)| s)
    }
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
