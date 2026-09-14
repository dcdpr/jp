//! Request and runtime usage snapshots for the Claude ACP flow.
//!
//! Repeated assistant messages update the entry identified by their message ID.
//! Runtime totals include work outside the main request stream and are retained
//! separately, never added to those entries.

use std::collections::BTreeMap;

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
    pub(super) fn observe(&mut self, message: &CreateMessagesResponse) {
        let (Some(id), Some(model), Some(usage)) = (&message.id, &message.model, &message.usage)
        else {
            return;
        };
        self.observe_usage(id, model, usage);
    }

    pub(super) fn observe_usage(&mut self, id: &str, model: &str, usage: &Usage) {
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

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
