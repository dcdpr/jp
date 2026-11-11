//! Test-only support for the Anthropic provider: a constructor that bypasses
//! the credential chain, the request-building seam, and the recorded-test
//! routes.

use std::result;

use jp_config::{
    model::id::ProviderId,
    providers::llm::{
        LlmProviderConfig,
        anthropic::{AnthropicConfig, AuthEntry, SubscriptionFlow},
    },
};
use jp_credentials::PROVIDER_ANTHROPIC;
use serde_json::Value;

use super::{
    Anthropic, BetaFeatures, CLAUDE_CODE_IDENTITY_LINE, IDENTITY_LINE_OVERRIDE, create_request,
    http, resolve,
};
use crate::{
    credential::Credential,
    error::Result,
    model::{ModelDetails, ReasoningDetails},
    provider::{
        ApiTestRoute, Provider, ProviderTestRoute, ProviderTestSupport, first_stored_profile,
        number_ids, replay_credential_env,
    },
    query::ChatQuery,
};

impl Anthropic {
    /// Build a provider around an explicit credential, bypassing the chain.
    ///
    /// Test seam: no environment or store access, no preflight.
    pub(crate) fn with_credential(config: &AnthropicConfig, credential: Credential) -> Self {
        Anthropic {
            config: config.clone(),
            beta: BetaFeatures(config.beta_headers.clone()),
            chain_on_max_tokens: config.chain_on_max_tokens,
            store: None,
            fixed_credential: Some(credential),
            client_cache: http::Clients::default(),
            seen_notices: resolve::SeenNotices::default(),
        }
    }

    /// Build the Anthropic wire request for `query` and serialize it to JSON
    /// without sending.
    /// Test-only seam for snapshotting request construction (notably compaction
    /// projection) across providers.
    pub(crate) fn request_value(&self, model: &ModelDetails, query: ChatQuery) -> Result<Value> {
        let bearer = matches!(self.fixed_credential, Some(Credential::Bearer(_)));
        let (request, ..) = create_request(model, query, true, &self.beta, bearer)?;
        Ok(serde_json::to_value(request)?)
    }
}

/// Anthropic's recorded-test routes.
///
/// Both reach the same host; a subscription request differs by its bearer
/// token, the Claude Code fingerprint, and the identity line leading its system
/// content.
pub(crate) static TEST_SUPPORT: AnthropicTestSupport = AnthropicTestSupport;

pub(crate) struct AnthropicTestSupport;

impl ProviderTestSupport for AnthropicTestSupport {
    fn api(&self) -> &'static dyn ProviderTestRoute {
        &API_ROUTE
    }

    fn subscription(&self) -> Option<&'static dyn ProviderTestRoute> {
        Some(&SUBSCRIPTION_ROUTE)
    }

    /// Strip the transport-mandated identity line, then erase what the model
    /// authored.
    fn project_request(&self, body: &Value) -> Value {
        let mut body = body.clone();

        strip_identity_line(&mut body);

        // Only an assistant turn holds the model's words. A `text` block in
        // `system` is JP's own prompt and stays compared.
        for message in body
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }

            for block in message
                .get_mut("content")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                erase_model_output(block);
            }
        }

        // `id` names a tool call, `tool_use_id` is how its result answers it.
        // Both are minted by the host.
        number_ids(&mut body, &["id", "tool_use_id"]);

        body
    }
}

/// Drop the identity line a bearer request has to lead with, and the block that
/// bounds it.
///
/// A request left holding nothing but those two ends up with an empty `system`,
/// while a request that never had a system prompt omits the field.
/// Both say the same thing and have to read the same way.
fn strip_identity_line(body: &mut Value) {
    let Some(system) = body.get_mut("system").and_then(Value::as_array_mut) else {
        return;
    };

    system.retain(|block| {
        let text = block
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        text != CLAUDE_CODE_IDENTITY_LINE && text != IDENTITY_LINE_OVERRIDE
    });

    if system.is_empty()
        && let Some(body) = body.as_object_mut()
    {
        body.remove("system");
    }
}

/// Erase what the model contributed to one content block of an assistant turn.
///
/// The Messages dialect files it three ways, and `input` here holds a tool
/// call's arguments.
/// Other dialects use the same key for the entire conversation, which is why
/// this reading belongs to Anthropic alone.
fn erase_model_output(block: &mut Value) {
    match block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "text" => replace_string(block, "text", "<model prose>"),

        // The thinking text is the model's; the signature is that text in a
        // form only the host can verify.
        "thinking" => {
            replace_string(block, "thinking", "<model prose>");
            replace_string(block, "signature", "<opaque>");
        }

        "tool_use" => {
            if let Some(input) = block.get_mut("input") {
                *input = Value::from("<model arguments>");
            }
        }

        _ => {}
    }
}

/// Replace the string at `key` with `placeholder`.
///
/// A missing key, or a value that is not a string, is left as it is.
fn replace_string(block: &mut Value, key: &str, placeholder: &str) {
    if let Some(value) = block.get_mut(key)
        && value.is_string()
    {
        *value = Value::from(placeholder);
    }
}

static SUBSCRIPTION_ROUTE: SubscriptionTestRoute = SubscriptionTestRoute;

/// The direct HTTP subscription route.
///
/// Recording authenticates exactly as a user's own machine does: through the
/// credential store, resolved by the same chain a real request walks.
/// Logging in is a one-time `jp provider llm auth login anthropic`, and refresh
/// and rotation are then the production code's problem rather than the
/// harness's.
pub(crate) struct SubscriptionTestRoute;

impl ProviderTestRoute for SubscriptionTestRoute {
    fn base_url(&self, config: &LlmProviderConfig) -> String {
        config.anthropic.base_url.clone()
    }

    fn set_base_url(&self, config: &mut LlmProviderConfig, url: String) {
        config.anthropic.base_url = url;
    }

    fn provider(
        &self,
        config: &LlmProviderConfig,
        recording: bool,
    ) -> result::Result<Box<dyn Provider>, String> {
        // A replayed cassette answers without authenticating, so the token only
        // has to exist. Reading the store on replay would make the suite
        // depend on the machine running it.
        if !recording {
            return Ok(Box::new(Anthropic::with_credential(
                &config.anthropic,
                Credential::Bearer("test-token".to_owned()),
            )));
        }

        let profile = first_stored_profile(PROVIDER_ANTHROPIC).ok_or(
            "recording needs a stored Claude subscription; run `jp provider llm auth login \
             anthropic` once",
        )?;

        let mut config = config.anthropic.clone();
        config.auth = vec![AuthEntry::Subscription(Some(profile))];
        config.subscription_flow = SubscriptionFlow::Direct;

        Anthropic::new(&config)
            .map(|provider| Box::new(provider) as Box<dyn Provider>)
            .map_err(|error| error.to_string())
    }

    fn model(&self) -> ModelDetails {
        API_ROUTE.model()
    }
}

static API_ROUTE: ApiTestRoute = ApiTestRoute {
    id: ProviderId::Anthropic,
    base_url: |config| config.anthropic.base_url.clone(),
    set_base_url: |config, url| config.anthropic.base_url = url,
    use_replay_credentials: |config| {
        config.anthropic.api_key_env = replay_credential_env().into();
    },
    model: || ModelDetails {
        id: "anthropic/claude-haiku-4-5".parse().unwrap(),
        display_name: None,
        context_window: Some(200_000),
        max_output_tokens: Some(64_000),
        reasoning: Some(ReasoningDetails::budgetted(1024, None)),
        knowledge_cutoff: None,
        deprecated: None,
        structured_output: None,
        prefill: None,
        subscription: None,
        features: vec!["interleaved-thinking", "context-editing"],
    },
};
