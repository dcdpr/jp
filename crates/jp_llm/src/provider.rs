pub(crate) mod api_key_chain;
// pub mod deepseek;
pub mod google;
// pub mod xai;
pub mod anthropic;
pub mod cerebras;
pub mod llamacpp;
pub mod mock;
pub mod ollama;
pub mod openai;
pub(crate) mod openai_compat;
pub mod openrouter;

use std::sync::atomic::{AtomicU64, Ordering};

use anthropic::Anthropic;
use async_trait::async_trait;
use cerebras::Cerebras;
use google::Google;
use jp_config::{
    model::id::{Name, ProviderId},
    providers::llm::LlmProviderConfig,
};
use llamacpp::Llamacpp;
use ollama::Ollama;
use openai::Openai;
use openrouter::Openrouter;

use crate::{
    error::Result, model::ModelDetails, provider::mock::MockProvider, query::ChatQuery,
    stream::EventStream,
};

#[async_trait]
pub trait Provider: Send + Sync {
    /// Get details of a model.
    async fn model_details(&self, name: &Name) -> Result<ModelDetails>;

    /// Get a list of available models.
    async fn models(&self) -> Result<Vec<ModelDetails>>;

    /// Perform a streaming chat completion.
    async fn chat_completion_stream(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
    ) -> Result<EventStream>;
}

/// Get a provider by ID.
///
/// Every provider is constructed from its configuration alone and validates its
/// own credentials during construction: an environment variable read, or a
/// credential-chain preflight against the store.
///
/// # Errors
///
/// Returns an error when the provider cannot possibly authenticate, e.g.
/// [`Error::MissingEnv`] when its API key environment variable is unset.
///
/// [`Error::MissingEnv`]: crate::Error::MissingEnv
pub fn get_provider(id: ProviderId, config: &LlmProviderConfig) -> Result<Box<dyn Provider>> {
    let provider: Box<dyn Provider> = match id {
        ProviderId::Anthropic => Box::new(Anthropic::new(&config.anthropic)?),
        ProviderId::Cerebras => Box::new(Cerebras::try_from(&config.cerebras)?),
        ProviderId::Google => Box::new(Google::try_from(&config.google)?),
        ProviderId::Llamacpp => Box::new(Llamacpp::try_from(&config.llamacpp)?),
        ProviderId::Ollama => Box::new(Ollama::try_from(&config.ollama)?),
        ProviderId::Openai => Box::new(Openai::new(&config.openai)?),
        ProviderId::Openrouter => Box::new(Openrouter::try_from(&config.openrouter)?),

        ProviderId::Deepseek => todo!(),
        ProviderId::Xai => todo!(),

        ProviderId::Test => Box::new(MockProvider::new(vec![])),
    };

    Ok(provider)
}

/// Validate that a provider is able to accept requests: credentials present,
/// configuration well-formed.
///
/// Synchronous and local: it reads the environment and, for a provider with a
/// credential chain, a store snapshot — never the network.
/// Constructing a provider implies this check passes: this *is*
/// [`get_provider`] with the client thrown away, packaged as an explicit seam
/// so callers can fail fast before starting side-effectful work (spawning
/// background tasks, loading attachments) that is wasted when the request can
/// never be sent.
///
/// # Errors
///
/// Returns the same errors as [`get_provider`], e.g. [`Error::MissingEnv`] when
/// the provider's API key environment variable is unset.
///
/// [`Error::MissingEnv`]: crate::Error::MissingEnv
pub fn preflight(id: ProviderId, config: &LlmProviderConfig) -> Result<()> {
    get_provider(id, config).map(drop)
}

/// Build the provider-native chat request for `query` and serialize it to JSON,
/// without sending it.
///
/// Test-only seam for snapshotting request construction across providers,
/// notably the effect of conversation compaction on each provider's message
/// serialization.
/// Each arm runs the same builder the live path uses, so the snapshot reflects
/// what would go on the wire.
#[cfg(test)]
pub(crate) fn build_request_value(
    id: ProviderId,
    config: &LlmProviderConfig,
    model: &ModelDetails,
    query: ChatQuery,
) -> Result<serde_json::Value> {
    match id {
        ProviderId::Anthropic => {
            // A fixed dummy credential: request construction is independent
            // of the credential's value, and tests must not read the
            // environment or the store.
            Anthropic::with_credential(
                &config.anthropic,
                crate::credential::Credential::ApiKey("test-api-key".to_owned()),
            )
            .request_value(model, query)
        }
        ProviderId::Cerebras => Cerebras::try_from(&config.cerebras)?.request_value(model, query),
        ProviderId::Google => Google::try_from(&config.google)?.request_value(model, query),
        ProviderId::Llamacpp => Llamacpp::try_from(&config.llamacpp)?.request_value(model, query),
        ProviderId::Ollama => Ollama::try_from(&config.ollama)?.request_value(model, query),
        ProviderId::Openai => Openai::with_credential(
            &config.openai,
            crate::credential::Credential::ApiKey("test-key".to_owned()),
        )
        .request_value(model, query),
        ProviderId::Openrouter => {
            Openrouter::try_from(&config.openrouter)?.request_value(model, query)
        }
        ProviderId::Test | ProviderId::Deepseek | ProviderId::Xai => {
            unreachable!("{id:?} is not part of the request snapshot suite")
        }
    }
}

/// Serialize a value to a temporary JSON file and return its path as a string.
///
/// Used by `trace!` fields to avoid dumping massive request payloads into the
/// log stream.
/// Each call writes a distinct, sequence-numbered file
/// (`{prefix}-{pid}-{seq}.json`) so successive requests within a single process
/// don't clobber each other's payloads.
pub(crate) fn trace_to_tmpfile(prefix: &str, value: &impl serde::Serialize) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{seq}.json", std::process::id()));
    match std::fs::write(
        &path,
        serde_json::to_string_pretty(value).unwrap_or_default(),
    ) {
        Ok(()) => path.display().to_string(),
        Err(_) => "<write failed>".to_owned(),
    }
}

/// One endpoint a provider's recorded tests can run against.
///
/// A provider reachable through more than one billing route implements this
/// once per route.
/// The harness drives every route identically, so a route is the only place
/// that knows an endpoint's URL, credentials, construction, or model catalog.
#[cfg(test)]
pub(crate) trait ProviderTestRoute: Sync {
    /// The upstream endpoint recordings forward to.
    fn base_url(&self, config: &LlmProviderConfig) -> String;

    /// Point the route at the local recording server.
    fn set_base_url(&self, config: &mut LlmProviderConfig, url: String);

    /// Replace credentials with values that satisfy construction on replay.
    ///
    /// A cassette answers without authenticating, so replay needs a credential
    /// that merely exists.
    /// The default suits a route that needs none.
    fn use_replay_credentials(&self, _config: &mut LlmProviderConfig) {}

    /// Build the provider for this route.
    ///
    /// While recording, the route supplies real credentials from wherever it
    /// natively keeps them, and reports how to refresh them when they are
    /// missing or stale.
    fn provider(
        &self,
        config: &LlmProviderConfig,
        recording: bool,
    ) -> std::result::Result<Box<dyn Provider>, String>;

    /// The model this route records against.
    fn model(&self) -> ModelDetails;
}

/// A provider's integration with the recorded test harness.
///
/// Implemented once per provider, beside that provider's implementation.
/// The defaults describe a provider that bills exactly one way.
#[cfg(test)]
pub(crate) trait ProviderTestSupport: Sync {
    /// The provider's metered API route.
    fn api(&self) -> &'static dyn ProviderTestRoute;

    /// The provider's subscription route, when it sells one.
    ///
    /// `None` skips the subscription suite for this provider.
    fn subscription(&self) -> Option<&'static dyn ProviderTestRoute> {
        None
    }

    /// Assert that rewriting a request for the subscription route preserves its
    /// meaning.
    ///
    /// Runs for each recorded chat request, before any fixture is involved.
    /// The default suits a provider with a single route, where nothing is
    /// rewritten.
    fn assert_rewrite_preserves_meaning(&self, _model: &ModelDetails, _query: ChatQuery) {}

    /// Reduce a recorded request body to what two recordings of one scenario
    /// must agree on.
    ///
    /// Two jobs, both needing to know the dialect, which is why they are one
    /// method:
    ///
    /// - Strip the dialect, since neither endpoint's shape is the meaning.
    /// - Erase what the model authored: its prose, the arguments it chose, its
    ///   reasoning, and whatever opaque token carries that reasoning back.
    ///
    /// Only the provider can tell those apart, because a key means different
    /// things in different dialects: `input` holds a tool call's arguments for
    /// one provider and the entire conversation for another.
    ///
    /// What survives is what JP decides — the order and roles of the
    /// conversation, the tools it offered, the results it returned, and the
    /// pairing between a call and its result.
    /// [`number_ids`] handles the last of those.
    ///
    /// The default panics rather than returning the body unchanged: two
    /// dialects comparing equal unprojected would be a coincidence.
    fn project_request(&self, _body: &serde_json::Value) -> serde_json::Value {
        panic!(
            "this provider records a second route but has no request projection; implement \
             `ProviderTestSupport::project_request` so its two recordings can be compared"
        )
    }
}

/// Replace the given fields' values with the order they first appear in.
///
/// Two recordings never share an id the host minted, so comparing them raw
/// fails on any scenario that replays one.
/// Numbering keeps a call and its result paired, which dropping the ids would
/// not.
///
/// Offered to [`ProviderTestSupport::project_request`]; the caller says which
/// of its fields hold an id.
#[cfg(test)]
pub(crate) fn number_ids(value: &mut serde_json::Value, keys: &[&str]) {
    fn walk(value: &mut serde_json::Value, keys: &[&str], seen: &mut Vec<String>) {
        match value {
            serde_json::Value::Array(values) => {
                for value in values {
                    walk(value, keys, seen);
                }
            }
            serde_json::Value::Object(object) => {
                for (key, value) in object.iter_mut() {
                    if keys.contains(&key.as_str())
                        && let Some(id) = value.as_str()
                    {
                        let position =
                            seen.iter().position(|seen| seen == id).unwrap_or_else(|| {
                                seen.push(id.to_owned());
                                seen.len() - 1
                            });

                        *value = serde_json::Value::from(format!("#{position}"));
                        continue;
                    }

                    walk(value, keys, seen);
                }
            }
            _ => {}
        }
    }

    walk(value, keys, &mut vec![]);
}

/// The stored profile a subscription recording authenticates with.
///
/// The lowest-named of however many are stored, so two runs land on the same
/// account without one being named.
#[cfg(test)]
pub(crate) fn first_stored_profile(provider: &str) -> Option<String> {
    let store = jp_credentials::CredentialStore::file_default().ok()?;
    let document = store.load().ok()?;
    let profiles = document.profiles(jp_credentials::CATEGORY_LLM, provider)?;

    profiles.keys().min().cloned()
}

/// The environment variable a replayed provider reads its credential from.
///
/// A cassette answers without authenticating, so any variable certain to be set
/// will do.
#[cfg(test)]
pub(crate) fn replay_credential_env() -> String {
    if cfg!(windows) { "USERNAME" } else { "USER" }.to_owned()
}

/// A provider's metered API route, as the facts that differ between providers.
#[cfg(test)]
pub(crate) struct ApiTestRoute {
    /// The provider this route builds.
    pub id: ProviderId,

    /// Read the route's endpoint out of configuration.
    pub base_url: fn(&LlmProviderConfig) -> String,

    /// Point the route's endpoint at a URL.
    ///
    /// A provider whose client appends an API version writes it here, so the
    /// recording server receives the same path the real endpoint would.
    pub set_base_url: fn(&mut LlmProviderConfig, String),

    /// Give the route a credential that satisfies construction on replay.
    pub use_replay_credentials: fn(&mut LlmProviderConfig),

    /// The model this route records against.
    pub model: fn() -> ModelDetails,
}

#[cfg(test)]
impl ProviderTestRoute for ApiTestRoute {
    fn base_url(&self, config: &LlmProviderConfig) -> String {
        (self.base_url)(config)
    }

    fn set_base_url(&self, config: &mut LlmProviderConfig, url: String) {
        (self.set_base_url)(config, url);
    }

    fn use_replay_credentials(&self, config: &mut LlmProviderConfig) {
        (self.use_replay_credentials)(config);
    }

    fn provider(
        &self,
        config: &LlmProviderConfig,
        _recording: bool,
    ) -> std::result::Result<Box<dyn Provider>, String> {
        // Built through the same entry point production uses, so a change to
        // construction cannot pass tests while breaking the binary.
        get_provider(self.id, config).map_err(|error| error.to_string())
    }

    fn model(&self) -> ModelDetails {
        (self.model)()
    }
}

/// A provider billed exactly one way.
#[cfg(test)]
pub(crate) struct ApiOnlyTestSupport(pub &'static ApiTestRoute);

#[cfg(test)]
impl ProviderTestSupport for ApiOnlyTestSupport {
    fn api(&self) -> &'static dyn ProviderTestRoute {
        self.0
    }
}

/// A provider whose test support has not been written.
#[cfg(test)]
pub(crate) struct UnsupportedTestRoute(pub ProviderId);

#[cfg(test)]
impl ProviderTestRoute for UnsupportedTestRoute {
    fn base_url(&self, _config: &LlmProviderConfig) -> String {
        unimplemented!("{} has no recorded test support", self.0)
    }

    fn set_base_url(&self, _config: &mut LlmProviderConfig, _url: String) {
        unimplemented!("{} has no recorded test support", self.0)
    }

    fn provider(
        &self,
        _config: &LlmProviderConfig,
        _recording: bool,
    ) -> std::result::Result<Box<dyn Provider>, String> {
        unimplemented!("{} has no recorded test support", self.0)
    }

    fn model(&self) -> ModelDetails {
        unimplemented!("{} has no recorded test support", self.0)
    }
}

#[cfg(test)]
static XAI_UNSUPPORTED: UnsupportedTestRoute = UnsupportedTestRoute(ProviderId::Xai);

#[cfg(test)]
static DEEPSEEK_UNSUPPORTED: UnsupportedTestRoute = UnsupportedTestRoute(ProviderId::Deepseek);

#[cfg(test)]
impl ProviderTestSupport for UnsupportedTestRoute {
    fn api(&self) -> &'static dyn ProviderTestRoute {
        match self.0 {
            ProviderId::Xai => &XAI_UNSUPPORTED,
            _ => &DEEPSEEK_UNSUPPORTED,
        }
    }
}

/// Every provider's integration with the recorded test harness.
///
/// The registry is exhaustive so a provider gaining a second billing route only
/// has to override [`ProviderTestSupport::subscription`]; nothing here or in
/// the harness changes.
#[cfg(test)]
pub(crate) fn provider_test_support(id: ProviderId) -> &'static dyn ProviderTestSupport {
    match id {
        ProviderId::Anthropic => &anthropic::TEST_SUPPORT,
        ProviderId::Cerebras => &cerebras::TEST_SUPPORT,
        ProviderId::Google => &google::TEST_SUPPORT,
        ProviderId::Llamacpp => &llamacpp::TEST_SUPPORT,
        ProviderId::Ollama => &ollama::TEST_SUPPORT,
        ProviderId::Openai => &openai::TEST_SUPPORT,
        ProviderId::Openrouter => &openrouter::TEST_SUPPORT,
        ProviderId::Test => &mock::TEST_SUPPORT,
        ProviderId::Xai => &XAI_UNSUPPORTED,
        ProviderId::Deepseek => &DEEPSEEK_UNSUPPORTED,
    }
}

#[cfg(test)]
#[path = "provider_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provider/compaction_request_tests.rs"]
mod compaction_request_tests;
