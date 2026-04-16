//! Parsing for the `web_fetch` tool's `options` map.
//!
//! See `.jp/mcp/tools/web/fetch.toml` for the user-facing schema.

use serde_json::{Map, Value};
use url::Url;

/// Fetch strategy for a given URL.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Strategy {
    /// Try the `.md` variant first; fall back to HTML on failure.
    #[default]
    Auto,
    /// Only fetch the `.md` variant. Error if unavailable.
    Markdown,
    /// Fetch HTML directly (legacy behavior; skips the `.md` probe).
    Html,
}

impl Strategy {
    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "markdown" | "md" => Some(Self::Markdown),
            "html" => Some(Self::Html),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct WebFetchOptions {
    default_strategy: Strategy,
    domains: Vec<DomainRule>,
}

#[derive(Debug)]
struct DomainRule {
    pattern: HostPattern,
    strategy: Strategy,
}

#[derive(Debug)]
enum HostPattern {
    Exact(String),
    /// Matches the suffix after `*.`, e.g. `*.foo.com` stores `foo.com` and
    /// matches both `foo.com` and `bar.foo.com`.
    Suffix(String),
}

impl HostPattern {
    fn parse(raw: &str) -> Self {
        let lower = raw.trim().trim_start_matches('.').to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("*.") {
            Self::Suffix(rest.to_owned())
        } else {
            Self::Exact(lower)
        }
    }

    fn matches(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        match self {
            Self::Exact(p) => host == *p,
            Self::Suffix(p) => host == *p || host.ends_with(&format!(".{p}")),
        }
    }
}

impl WebFetchOptions {
    pub(super) fn parse(options: &Map<String, Value>) -> Self {
        let default_strategy = options
            .get("strategy")
            .and_then(Value::as_str)
            .and_then(Strategy::parse)
            .unwrap_or_default();

        let domains = options
            .get("domains")
            .and_then(Value::as_object)
            .map(parse_domain_rules)
            .unwrap_or_default();

        Self {
            default_strategy,
            domains,
        }
    }

    /// Pick the strategy to use for the given URL.
    ///
    /// The first matching domain rule wins; otherwise the default is returned.
    pub(super) fn pick_strategy(&self, url: &Url) -> Strategy {
        let Some(host) = url.host_str() else {
            return self.default_strategy;
        };

        self.domains
            .iter()
            .find(|rule| rule.pattern.matches(host))
            .map_or(self.default_strategy, |rule| rule.strategy)
    }
}

fn parse_domain_rules(raw: &Map<String, Value>) -> Vec<DomainRule> {
    raw.iter()
        .filter_map(|(key, value)| {
            let strategy = value.as_str().and_then(Strategy::parse)?;
            Some(DomainRule {
                pattern: HostPattern::parse(key),
                strategy,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
