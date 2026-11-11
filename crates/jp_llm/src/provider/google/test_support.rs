//! Test-only support for the Google provider: the request-building seam and the
//! recorded-test route.

use jp_config::model::id::ProviderId;
use serde_json::Value;

use super::{Google, create_request};
use crate::{
    error::Result,
    model::{ModelDetails, ReasoningDetails},
    provider::{ApiOnlyTestSupport, ApiTestRoute, replay_credential_env},
    query::ChatQuery,
};

impl Google {
    /// Build the Google wire request for `query` and serialize it to JSON
    /// without sending.
    /// Test-only seam for snapshotting request construction (notably compaction
    /// projection) across providers.
    #[expect(
        clippy::unused_self,
        reason = "uniform per-provider seam; only some providers read instance state"
    )]
    pub(crate) fn request_value(&self, model: &ModelDetails, query: ChatQuery) -> Result<Value> {
        let (request, _) = create_request(model, query)?;
        Ok(serde_json::to_value(request)?)
    }
}

/// Google's recorded-test route.
pub(crate) static TEST_SUPPORT: ApiOnlyTestSupport = ApiOnlyTestSupport(&API_ROUTE);

static API_ROUTE: ApiTestRoute = ApiTestRoute {
    id: ProviderId::Google,
    base_url: |config| config.google.base_url.clone(),
    // The client addresses the API version as part of the host, so the
    // recording server has to be reachable at the same path.
    set_base_url: |config, url| config.google.base_url = format!("{url}/v1beta"),
    use_replay_credentials: |config| {
        config.google.api_key_env = replay_credential_env().into();
    },
    model: || ModelDetails {
        id: "google/gemini-2.5-flash-lite".parse().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(512, Some(24576))),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        prefill: None,
        subscription: None,
        features: vec![],
    },
};
