//! Request and runtime usage snapshots for the Claude ACP flow.
//!
//! Repeated assistant messages update the entry identified by their message ID.
//! Runtime totals include work outside the main request stream and are retained
//! separately, never added to those entries.

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use async_anthropic::types::{CreateMessagesResponse, Usage};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Default)]
pub(super) struct UsageLedger {
    requests: BTreeMap<String, RequestUsage>,
    runtime: Option<RuntimeUsage>,
}

#[derive(Debug, Serialize)]
struct RequestUsage {
    model: String,
    #[serde(flatten)]
    usage: Usage,
}

/// Native SDK aggregates, not increments to the main request counters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct RuntimeUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub model_usage: BTreeMap<String, ModelUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModelUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_creation_input_tokens: Option<u64>,
    #[serde(default, rename = "costUSD", skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u64>,
}

impl UsageLedger {
    /// Record `message`'s usage; returns whether it is the first seen for its
    /// request.
    pub(super) fn observe(&mut self, message: &CreateMessagesResponse) -> bool {
        let (Some(id), Some(model), Some(usage)) = (&message.id, &message.model, &message.usage)
        else {
            return false;
        };
        self.observe_usage(id, model, usage)
    }

    /// Record a request's usage; returns whether it is the first seen for the
    /// request.
    pub(super) fn observe_usage(&mut self, id: &str, model: &str, usage: &Usage) -> bool {
        let new = !self.requests.contains_key(id);
        let entry = self
            .requests
            .entry(id.to_owned())
            .or_insert_with(|| RequestUsage {
                model: model.to_owned(),
                usage: usage.clone(),
            });
        entry.usage.input_tokens = usage.input_tokens.or(entry.usage.input_tokens);
        // A complete assistant block can repeat the initial usage after the
        // terminal message_delta has reported the final generated count.
        entry.usage.output_tokens = entry.usage.output_tokens.max(usage.output_tokens);
        entry.usage.cache_creation_input_tokens = usage
            .cache_creation_input_tokens
            .or(entry.usage.cache_creation_input_tokens);
        entry.usage.cache_read_input_tokens = usage
            .cache_read_input_tokens
            .or(entry.usage.cache_read_input_tokens);
        if let Some(creation) = &usage.cache_creation {
            entry.usage.cache_creation = Some(creation.clone());
        }
        new
    }

    pub(super) fn set_runtime(&mut self, runtime: RuntimeUsage) {
        if runtime.usage.is_some()
            || !runtime.model_usage.is_empty()
            || runtime.estimated_cost_usd.is_some()
        {
            self.runtime = Some(runtime);
        }
    }

    /// A cumulative snapshot; consumers replace earlier snapshots for this
    /// native session instead of summing repeated observations.
    pub(super) fn snapshot(&self, session: &str) -> Value {
        let mut value = json!({"native_session_id":session,"requests":self.requests});
        if let Some(runtime) = &self.runtime {
            value["runtime"] = json!(runtime);
        }
        value
    }
}

/// The shortest cache lifetime Claude Code requests.
///
/// A request further than this from the one before it may find that request's
/// cache entry expired, so a smaller read is no evidence of a bug.
const SHORTEST_CACHE_LIFETIME: Duration = Duration::from_mins(5);

/// Checks that each main request in a connection reads from the prompt cache at
/// least what the request before it read and wrote.
///
/// Within one connection the conversation only grows: the history is fixed when
/// the connection opens, and compaction, edits, and forks happen between
/// connections.
/// So a request reading less than its predecessor cached means a changed
/// prefix, unless the model changed or the entry may have expired.
#[derive(Debug, Default)]
pub(super) struct CacheContinuity {
    previous: Option<CachedRequest>,
}

#[derive(Debug)]
struct CachedRequest {
    model: String,
    /// What the request read from the cache plus what it wrote to it.
    cached: u64,
    at: Instant,
}

impl CacheContinuity {
    /// Record a new main request, sent at `at`, and describe the miss when it
    /// read less than the previous request cached.
    ///
    /// A request without cache counts is skipped, and leaves the previous
    /// request as the one the next is compared with.
    pub(super) fn check(&mut self, model: &str, usage: &Usage, at: Instant) -> Option<String> {
        let (Some(read), Some(written)) = (
            usage.cache_read_input_tokens,
            usage.cache_creation_input_tokens,
        ) else {
            return None;
        };
        let current = CachedRequest {
            model: model.to_owned(),
            cached: read + written,
            at,
        };
        let previous = self.previous.replace(current)?;
        let comparable = previous.model == model
            && at.saturating_duration_since(previous.at) < SHORTEST_CACHE_LIFETIME;
        (comparable && read < previous.cached).then(|| {
            format!(
                "prompt cache not reused within the turn: read {read} cached tokens, the previous \
                 request cached {}; please report this to JP",
                previous.cached
            )
        })
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
