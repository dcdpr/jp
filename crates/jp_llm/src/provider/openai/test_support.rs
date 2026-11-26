//! Test-only support for the OpenAI provider: constructors that bypass the
//! credential chain, the request-building seam, and the recorded-test routes.

use std::{
    result,
    sync::{Arc, Mutex},
    time::Duration,
};

use jp_config::providers::llm::{AuthEntry, LlmProviderConfig, openai::OpenaiConfig};
use jp_credentials::{CredentialStore, PROVIDER_OPENAI};
use serde_json::Value;
use tokio::{io::AsyncReadExt as _, net::TcpStream};

use super::{
    ModelResponse, Openai, PROVIDER, RESET_PROPAGATION_BUDGET, RESET_RETRY_INTERVAL,
    create_request, map_model, parity, resolve,
};
use crate::{
    credential::Credential,
    error::Result,
    model::ModelDetails,
    provider::{
        ApiTestRoute, DynProvider, ProviderTestRoute, ProviderTestSupport, first_stored_profile,
        notices::SeenNotices, replay_credential_env,
    },
    query::ChatQuery,
};

impl Openai {
    /// Build a provider around an explicit credential, bypassing the chain.
    ///
    /// Test seam: no environment or store access, no preflight.
    // qual:allow(dry, boilerplate) reason: "copies fields out of a borrowed config; no `From` conversion applies"
    pub(crate) fn with_credential(config: &OpenaiConfig, credential: Credential) -> Self {
        Self {
            config: config.clone(),
            store: None,
            fixed_credential: Some((credential, resolve::Attribution::default())),
            base_url: config.base_url.clone(),
            codex_base_url: config.codex_base_url.clone(),
            reset_budget: RESET_PROPAGATION_BUDGET,
            reset_interval: RESET_RETRY_INTERVAL,
            pending_redemption: Arc::new(Mutex::new(None)),
            client_cache: Arc::new(Mutex::new(None)),
            seen_notices: SeenNotices::default(),
        }
    }

    /// Build a provider that resolves its chain against `store`.
    ///
    /// Test seam: the chain walks an in-memory store instead of the user's, and
    /// no preflight runs.
    pub(crate) fn with_store(config: &OpenaiConfig, store: CredentialStore) -> Self {
        Self {
            store: Some(store),
            fixed_credential: None,
            ..Self::with_credential(config, Credential::ApiKey(String::new()))
        }
    }

    /// Override the rate-limit reset retry budget and polling interval.
    pub(crate) fn with_reset_timing(mut self, budget: Duration, interval: Duration) -> Self {
        self.reset_budget = budget;
        self.reset_interval = interval;
        self
    }

    /// Build a provider around an explicit subscription credential.
    ///
    /// Test seam for recording and replaying the subscription endpoint without
    /// reading the user's credential store.
    pub(crate) fn with_subscription_credential(
        config: &OpenaiConfig,
        token: String,
        account_id: String,
    ) -> Self {
        let mut provider = Self::with_credential(config, Credential::Bearer(token));
        let Some((_, attribution)) = &mut provider.fixed_credential else {
            unreachable!("with_credential always sets a fixed credential");
        };
        attribution.account_id = Some(account_id);
        provider
    }

    /// Build the OpenAI wire request for `query` and serialize it to JSON
    /// without sending.
    /// Test-only seam for snapshotting request construction (notably compaction
    /// projection) across providers.
    #[expect(
        clippy::unused_self,
        reason = "uniform per-provider seam; only some providers read instance state"
    )]
    pub(crate) fn request_value(&self, model: &ModelDetails, query: ChatQuery) -> Result<Value> {
        let (request, ..) = create_request(model, query)?;
        Ok(serde_json::to_value(request)?)
    }
}

pub(crate) fn catalog_model_details(name: &str) -> ModelDetails {
    map_model(ModelResponse::named(name.to_owned())).expect("a valid OpenAI model id")
}

/// `OpenAI`'s recorded-test routes (API key and `ChatGPT` subscription).
pub(crate) static TEST_SUPPORT: OpenaiTestSupport = OpenaiTestSupport;

pub(crate) struct OpenaiTestSupport;

impl ProviderTestSupport for OpenaiTestSupport {
    fn api(&self) -> &'static dyn ProviderTestRoute {
        &API_ROUTE
    }

    fn subscription(&self) -> Option<&'static dyn ProviderTestRoute> {
        Some(&SUBSCRIPTION_ROUTE)
    }

    fn assert_rewrite_preserves_meaning(&self, model: &ModelDetails, query: ChatQuery) {
        parity::assert_rewrite_preserves_meaning(model, query);
    }

    fn project_request(&self, body: &Value) -> Value {
        parity::project_body(body)
    }
}

static API_ROUTE: ApiTestRoute = ApiTestRoute {
    id: PROVIDER,
    base_url: |config| config.openai.base_url.clone(),
    set_base_url: |config, url| config.openai.base_url = url,
    use_replay_credentials: |config| {
        config.openai.api_key_env = replay_credential_env().into();
    },
    // Use `SHARED_TEST_MODEL` from the catalog so the API and subscription
    // routes record with identical model settings (reasoning, caching, and
    // sampling).
    model: || catalog_model_details(SHARED_TEST_MODEL),
};

/// The model both recorded routes use.
///
/// Served by a subscription and by the API, and the cheapest model that carries
/// the full GPT-5.6 feature set.
pub(crate) const SHARED_TEST_MODEL: &str = "gpt-5.6-luna";

static SUBSCRIPTION_ROUTE: SubscriptionTestRoute = SubscriptionTestRoute;

/// The `ChatGPT` subscription route.
///
/// Recording authenticates exactly as a user's own machine does: through the
/// credential store, resolved by the same chain a real request walks.
/// Logging in is a one-time `jp provider llm auth login openai`, and refresh
/// and rotation are then the production code's problem rather than the
/// harness's.
pub(crate) struct SubscriptionTestRoute;

impl ProviderTestRoute for SubscriptionTestRoute {
    fn base_url(&self, config: &LlmProviderConfig) -> String {
        config.openai.codex_base_url.clone()
    }

    fn set_base_url(&self, config: &mut LlmProviderConfig, url: String) {
        // The recording proxy forwards only the request path to the upstream
        // host and drops any path prefix on the target URL. Include
        // `/backend-api/codex` in `codex_base_url` so requests reach
        // `/backend-api/codex/responses`.
        config.openai.codex_base_url = format!("{url}/backend-api/codex");
    }

    fn provider(
        &self,
        config: &LlmProviderConfig,
        recording: bool,
    ) -> result::Result<Box<dyn DynProvider>, String> {
        // A replayed cassette answers without authenticating, so the token only
        // has to exist. Reading the store on replay would make the suite
        // depend on the machine running it.
        if !recording {
            return Ok(Box::new(Openai::with_subscription_credential(
                &config.openai,
                "test-token".to_owned(),
                "test-account".to_owned(),
            )));
        }

        let profile = first_stored_profile(PROVIDER_OPENAI).ok_or(
            "recording needs a stored ChatGPT subscription; run `jp provider llm auth login \
             openai` once",
        )?;

        let mut config = config.openai.clone();
        config.auth = vec![AuthEntry::Subscription(Some(profile))];

        Openai::new(&config)
            .map(|provider| Box::new(provider) as Box<dyn DynProvider>)
            .map_err(|error| error.to_string())
    }

    fn model(&self) -> ModelDetails {
        catalog_model_details(SHARED_TEST_MODEL)
    }
}

/// Read one HTTP request's target and body from `socket`.
pub(crate) async fn read_request(socket: &mut TcpStream) -> (String, String) {
    let (head, body) = read_raw_request(socket).await;
    let target = head
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();

    (target, body)
}

/// Read one HTTP request's head (request line and headers) and body from
/// `socket`.
pub(crate) async fn read_raw_request(socket: &mut TcpStream) -> (String, String) {
    let mut raw = vec![];
    let mut chunk = [0u8; 1024];

    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        raw.extend_from_slice(&chunk[..read]);

        let text = String::from_utf8_lossy(&raw).to_string();
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };

        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);

        if body.len() >= length || read == 0 {
            return (head.to_owned(), body.to_owned());
        }
    }
}
