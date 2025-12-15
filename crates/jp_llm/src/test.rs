use std::{panic, sync::Arc};

use futures::TryStreamExt as _;
use jp_config::{
    AppConfig, Config as _, PartialAppConfig, ToPartial as _,
    assistant::tool_choice::ToolChoice,
    conversation::tool::{RunMode, ToolParameterConfig},
    model::{
        id::{ModelIdConfig, Name, PartialModelIdConfig, PartialModelIdOrAliasConfig, ProviderId},
        parameters::{PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningEffort},
    },
    providers::llm::LlmProviderConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ToolCallResponse},
    stream::ConversationEventWithConfig,
    thread::{Thread, ThreadBuilder},
};
use jp_test::mock::{Snap, Vcr};
use schemars::Schema;
use time::macros::datetime;

use crate::{
    event::Event,
    model::{ModelDetails, ReasoningDetails},
    provider::get_provider,
    query::{ChatQuery, StructuredQuery},
    stream::aggregator::chunk::EventAggregator,
    tool::ToolDefinition,
};

pub enum TestRequest {
    /// A chat completion request.
    Chat {
        stream: bool,
        model: ModelDetails,
        query: ChatQuery,
        #[expect(clippy::type_complexity)]
        assert: Arc<dyn Fn(&[Vec<Event>])>,
    },

    /// A structured completion request.
    Structured {
        model: ModelDetails,
        query: StructuredQuery,
        #[expect(clippy::type_complexity)]
        assert: Arc<dyn Fn(&[Result<serde_json::Value, crate::Error>])>,
    },

    /// List all models.
    Models {
        #[expect(clippy::type_complexity)]
        assert: Arc<dyn Fn(&[ModelDetails])>,
    },

    /// A model details request.
    ModelDetails {
        name: String,
        #[expect(clippy::type_complexity)]
        assert: Arc<dyn Fn(&[ModelDetails])>,
    },

    /// A tool call response, given the same ID as the last tool call request in
    /// the stream.
    ToolCallResponse {
        result: Result<String, String>,
        panic_on_missing_request: bool,
    },

    /// A generic closure that takes a list of current events in the
    /// conversation stream, and returns a new request.
    #[expect(clippy::type_complexity)]
    Func(Box<dyn FnOnce(&[ConversationEventWithConfig]) -> Option<TestRequest>>),
}

impl TestRequest {
    pub fn func(f: impl FnOnce(&[ConversationEventWithConfig]) -> Option<Self> + 'static) -> Self {
        Self::Func(Box::new(f))
    }

    pub fn chat(provider: ProviderId) -> Self {
        Self::Chat {
            stream: false,
            model: test_model_details(provider),
            query: ChatQuery {
                thread: ThreadBuilder::new()
                    .with_events(ConversationStream::new({
                        let mut cfg = PartialAppConfig::empty();
                        cfg.conversation.tools.defaults.run = Some(RunMode::Ask);
                        cfg.assistant.model.parameters.reasoning =
                            Some(PartialReasoningConfig::Off);
                        cfg.assistant.model.id = PartialModelIdConfig {
                            provider: Some(provider),
                            name: Some("test".parse().unwrap()),
                        }
                        .into();

                        AppConfig::from_partial(cfg).unwrap()
                    }))
                    .build()
                    .unwrap(),
                tools: vec![],
                tool_choice: ToolChoice::default(),
                tool_call_strict_mode: false,
            },
            assert: Arc::new(|_| {}),
        }
    }

    #[expect(dead_code)]
    pub fn structured(provider: ProviderId) -> Self {
        Self::Structured {
            model: test_model_details(provider),
            query: StructuredQuery::new(
                true.into(),
                ThreadBuilder::new()
                    .with_events(ConversationStream::new({
                        let mut cfg = PartialAppConfig::empty();
                        cfg.conversation.tools.defaults.run = Some(RunMode::Ask);
                        cfg.assistant.model.id = PartialModelIdConfig {
                            provider: Some(provider),
                            name: Some("test".parse().unwrap()),
                        }
                        .into();

                        AppConfig::from_partial(cfg).unwrap()
                    }))
                    .build()
                    .unwrap(),
            ),
            assert: Arc::new(|_| {}),
        }
    }

    pub fn tool_call_response(result: Result<&str, &str>, panic_on_missing_request: bool) -> Self {
        Self::ToolCallResponse {
            result: result.map(Into::into).map_err(Into::into),
            panic_on_missing_request,
        }
    }

    #[expect(dead_code)]
    pub fn models() -> Self {
        Self::Models {
            assert: Arc::new(|_| {}),
        }
    }

    pub fn stream(mut self, stream: bool) -> Self {
        if let Self::Chat { stream: s, .. } = &mut self {
            *s = stream;
        }

        self
    }

    pub fn model(mut self, model: ModelIdConfig) -> Self {
        let Some(thread) = self.as_thread_mut() else {
            return self;
        };

        let mut delta = PartialAppConfig::empty();
        delta.assistant.model.id = PartialModelIdOrAliasConfig::Id(model.to_partial());
        thread.events.add_config_delta(delta);

        match &mut self {
            Self::Chat { model: m, .. } | Self::Structured { model: m, .. } => m.id = model,
            _ => {}
        }

        self
    }

    pub fn enable_reasoning(mut self) -> Self {
        let Some(thread) = self.as_thread_mut() else {
            return self;
        };

        let mut delta = PartialAppConfig::empty();
        delta.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Low),
                exclude: Some(false),
            },
        ));

        thread.events.add_config_delta(delta);
        self
    }

    pub fn reasoning(mut self, reasoning: Option<PartialReasoningConfig>) -> Self {
        let Some(thread) = self.as_thread_mut() else {
            return self;
        };

        let mut delta = PartialAppConfig::empty();
        delta.assistant.model.parameters.reasoning = reasoning;

        thread.events.add_config_delta(delta);
        self
    }

    pub fn event(mut self, event: impl Into<ConversationEvent>) -> Self {
        let Some(thread) = self.as_thread_mut() else {
            return self;
        };

        thread.events.push(event.into());
        self
    }

    pub fn chat_request(self, request: impl Into<ChatRequest>) -> Self {
        self.event(request.into())
    }

    pub fn tool_choice(mut self, choice: ToolChoice) -> Self {
        if let TestRequest::Chat { query, .. } = &mut self {
            query.tool_choice = choice;
        }

        self
    }

    #[expect(dead_code)]
    pub fn schema(self, schema: impl Into<Schema>) -> Self {
        match self {
            Self::Structured {
                model,
                query,
                assert,
            } => Self::Structured {
                model,
                query: StructuredQuery::new(schema.into(), query.thread),
                assert,
            },
            _ => self,
        }
    }

    pub fn tool_choice_fn(self, name: impl Into<String>) -> Self {
        self.tool_choice(ToolChoice::Function(name.into()))
    }

    pub fn tool<S: Into<String>, I: IntoIterator<Item = (&'static str, ToolParameterConfig)>>(
        mut self,
        name: S,
        definitions: I,
    ) -> Self {
        if let Self::Chat { query, .. } = &mut self {
            query.tools.push(ToolDefinition {
                name: name.into(),
                description: None,
                parameters: definitions
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect(),
            });
        }

        self
    }

    pub fn tool_call_strict_mode(mut self, strict: bool) -> Self {
        if let Self::Chat { query, .. } = &mut self {
            query.tool_call_strict_mode = strict;
        }

        self
    }

    #[expect(dead_code)]
    pub fn assert_chat(mut self, assert: impl Fn(&[Vec<Event>]) + 'static) -> Self {
        if let Self::Chat { assert: a, .. } = &mut self {
            *a = Arc::new(assert);
        }

        self
    }

    #[expect(dead_code)]
    pub fn assert_structured(
        mut self,
        assert: impl Fn(&[Result<serde_json::Value, crate::Error>]) + 'static,
    ) -> Self {
        if let Self::Structured { assert: a, .. } = &mut self {
            *a = Arc::new(assert);
        }

        self
    }

    #[expect(dead_code)]
    pub fn assert_models(mut self, assert: impl Fn(&[ModelDetails]) + 'static) -> Self {
        if let Self::Models { assert: a, .. } = &mut self {
            *a = Arc::new(assert);
        }

        self
    }

    pub fn as_thread(&self) -> Option<&Thread> {
        match self {
            Self::Chat { query, .. } => Some(&query.thread),
            Self::Structured { query, .. } => Some(&query.thread),
            _ => None,
        }
    }

    pub fn as_thread_mut(&mut self) -> Option<&mut Thread> {
        match self {
            Self::Chat { query, .. } => Some(&mut query.thread),
            Self::Structured { query, .. } => Some(&mut query.thread),
            _ => None,
        }
    }
}

impl std::fmt::Debug for TestRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Chat {
                stream,
                model,
                query,
                ..
            } => f
                .debug_struct("Chat")
                .field("stream", stream)
                .field("model", model)
                .field("query", query)
                .finish(),
            Self::Structured { model, query, .. } => f
                .debug_struct("Structured")
                .field("model", model)
                .field("query", query)
                .finish(),
            Self::Models { .. } => f.debug_struct("Models").finish(),
            Self::ModelDetails { .. } => f.debug_struct("ModelDetails").finish(),
            Self::ToolCallResponse {
                result,
                panic_on_missing_request,
            } => f
                .debug_struct("ToolCallResponse")
                .field("panic_on_missing_request", &panic_on_missing_request)
                .field("result", &result)
                .finish(),
            Self::Func(_) => f.debug_struct("Fn").finish(),
        }
    }
}

pub async fn run_test(
    provider: ProviderId,
    test_name: impl AsRef<str>,
    requests: impl IntoIterator<Item = TestRequest>,
) -> jp_test::Result {
    crate::test::run_chat_completion(
        test_name,
        env!("CARGO_MANIFEST_DIR"),
        provider,
        LlmProviderConfig::default(),
        requests.into_iter().collect(),
    )
    .await
}

#[expect(clippy::too_many_lines)]
pub async fn run_chat_completion(
    test_name: impl AsRef<str>,
    manifest_dir: &'static str,
    provider_id: ProviderId,
    mut config: LlmProviderConfig,
    requests: Vec<TestRequest>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let vcr = Vcr::new(
        match provider_id {
            ProviderId::Anthropic => config.anthropic.base_url.clone(),
            ProviderId::Google => config.google.base_url.clone(),
            ProviderId::Llamacpp => config.llamacpp.base_url.clone(),
            ProviderId::Ollama => config.ollama.base_url.clone(),
            ProviderId::Openai => config.openai.base_url.clone(),
            ProviderId::Openrouter => config.openrouter.base_url.clone(),
            _ => String::new(),
        },
        manifest_dir,
    )
    .with_fixture_suffix(&provider_id.as_str());

    vcr.cassette(
        test_name.as_ref(),
        |rule| {
            rule.filter(|when| {
                when.any_request();
            });
        },
        |recording, url| async move {
            match provider_id {
                ProviderId::Anthropic => config.anthropic.base_url = url,
                ProviderId::Google => config.google.base_url = format!("{url}/v1beta"),
                ProviderId::Llamacpp => config.llamacpp.base_url = url,
                ProviderId::Ollama => config.ollama.base_url = url,
                ProviderId::Openai => config.openai.base_url = url,
                ProviderId::Openrouter => config.openrouter.base_url = url,
                _ => {}
            }

            if !recording {
                // dummy api key value when replaying a cassette
                match provider_id {
                    ProviderId::Anthropic => config.anthropic.api_key_env = "USER".to_owned(),
                    ProviderId::Google => config.google.api_key_env = "USER".to_owned(),
                    ProviderId::Openai => config.openai.api_key_env = "USER".to_owned(),
                    ProviderId::Openrouter => config.openrouter.api_key_env = "USER".to_owned(),
                    _ => {}
                }
            }

            let provider = get_provider(provider_id, &config).unwrap();
            let has_structured_request = requests
                .iter()
                .any(|v| matches!(v, TestRequest::Structured { .. }));
            let has_chat_request = requests
                .iter()
                .any(|v| matches!(v, TestRequest::Chat { .. }));
            let has_model_details_request = requests
                .iter()
                .any(|v| matches!(v, TestRequest::ModelDetails { .. }));
            let has_models_request = requests
                .iter()
                .any(|v| matches!(v, TestRequest::Models { .. }));

            // Tracked to save in a snapshot at the end of the test for easier
            // debugging.
            let mut conversation_stream = None;

            let mut all_events = vec![];
            let mut history = vec![];
            let mut structured_history = vec![];
            let mut model_details = vec![];
            let mut models = vec![];

            for (index, mut request) in requests.into_iter().enumerate() {
                all_events.push(vec![]);

                if let TestRequest::ToolCallResponse {
                    result,
                    panic_on_missing_request,
                } = request
                {
                    request = TestRequest::func(move |history| {
                        let last = match history.last() {
                            Some(event) => event,
                            None if panic_on_missing_request => {
                                panic!(
                                    "`ToolCallResponse` must be preceded by a `ToolCallRequest`: \
                                     {history:#?}"
                                )
                            }
                            None => return None,
                        };

                        let tool_call = match last.as_tool_call_request() {
                            Some(tool_call) => tool_call,
                            None if panic_on_missing_request => {
                                panic!(
                                    "`ToolCallResponse` must be preceded by a `ToolCallRequest`: \
                                     {history:#?}. Last: {last:#?}"
                                )
                            }
                            None => return None,
                        };

                        Some(TestRequest::chat(provider_id).event(ToolCallResponse {
                            id: tool_call.id.clone(),
                            result,
                        }))
                    });
                }

                let mut maybe_request = Some(request);
                while let Some(TestRequest::Func(func)) = maybe_request {
                    maybe_request = func(&history);
                }

                request = match maybe_request {
                    Some(request) => request,
                    // If the request is `None`, The user decided not to execute
                    // this test request, so we skip it.
                    None => continue,
                };

                let config = match &request {
                    TestRequest::Chat { query, .. } => {
                        query.thread.events.config().unwrap().to_partial()
                    }
                    TestRequest::Structured { query, .. } => {
                        query.thread.events.config().unwrap().to_partial()
                    }
                    TestRequest::Models { .. } | TestRequest::ModelDetails { .. } => {
                        PartialAppConfig::empty()
                    }
                    TestRequest::ToolCallResponse { .. } | TestRequest::Func(_) => {
                        unreachable!("resolved at start of loop")
                    }
                };

                // 1. First, we append the new conversation events to the
                //    history.
                if let Some(thread) = request.as_thread() {
                    history.extend(
                        thread
                            .events
                            .clone()
                            .into_iter()
                            .map(|mut v| {
                                v.event.timestamp = datetime!(2020-01-01 0:00 utc).into();
                                v
                            })
                            .collect::<Vec<_>>(),
                    );
                }

                // 2. Then, we create a new eventstream with the history.
                if let Some(thread) = request.as_thread_mut() {
                    thread.events.clear();
                    thread.events.extend(history.clone());
                    conversation_stream = Some(thread.events.clone());
                }

                // 3. Then we run the query, and collect the new events.
                match request {
                    TestRequest::Chat {
                        stream,
                        model,
                        query,
                        assert,
                    } => {
                        let mut agg = EventAggregator::new();
                        let events = if stream {
                            provider
                                .chat_completion_stream(&model, query)
                                .await
                                .unwrap()
                                .try_collect()
                                .await
                                .unwrap()
                        } else {
                            provider.chat_completion(&model, query).await.unwrap()
                        };

                        for mut event in events {
                            if let Event::Part { event, .. } = &mut event {
                                event.timestamp = datetime!(2020-01-01 0:00 utc).into();
                            }

                            all_events[index].push(event.clone());

                            for event in agg.ingest(event) {
                                if let Event::Part { event, .. } = event {
                                    if let Some(stream) = conversation_stream.as_mut() {
                                        stream.push(event.clone());
                                    }

                                    history.push(ConversationEventWithConfig {
                                        event,
                                        config: config.clone(),
                                    });
                                }
                            }
                        }

                        assert(&all_events);
                    }
                    TestRequest::Structured {
                        model,
                        query,
                        assert,
                    } => {
                        let value = provider.structured_completion(&model, query).await;
                        structured_history.push(value);
                        assert(&structured_history);
                    }
                    TestRequest::Models { assert } => {
                        let value = provider.models().await.unwrap();
                        models.extend(value);
                        assert(&models);
                    }
                    TestRequest::ModelDetails { name, assert } => {
                        let name: Name = name.parse().unwrap();
                        let value = provider.model_details(&name).await.unwrap();
                        model_details.push(value);
                        assert(&model_details);
                    }
                    TestRequest::ToolCallResponse { .. } | TestRequest::Func(_) => {
                        unreachable!("resolved at start of loop")
                    }
                }
            }

            let mut outputs = vec![];
            if has_chat_request {
                outputs.extend(vec![
                    ("", Snap::debug(all_events)),
                    ("conversation_stream", Snap::json(conversation_stream)),
                ]);
            }

            if has_structured_request {
                let out = structured_history
                    .into_iter()
                    .map(|v| match v {
                        Ok(value) => value,
                        Err(error) => format!("Error::{error:?}").into(),
                    })
                    .collect::<Vec<_>>();

                outputs.push(("structured_outputs", Snap::json(out)));
            }

            if has_model_details_request {
                outputs.push(("model_details", Snap::debug(model_details)));
            }

            if has_models_request {
                outputs.push(("models", Snap::debug(models)));
            }

            outputs
        },
    )
    .await?;

    Ok(())
}

pub(crate) fn test_model_details(id: ProviderId) -> ModelDetails {
    match id {
        ProviderId::Anthropic => ModelDetails {
            id: "anthropic/claude-haiku-4-5".parse().unwrap(),
            display_name: None,
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(1024, None)),
            knowledge_cutoff: None,
            deprecated: None,
            features: vec!["interleaved-thinking", "context-editing"],
        },
        ProviderId::Google => ModelDetails {
            id: "google/gemini-2.5-flash-lite".parse().unwrap(),
            display_name: None,
            context_window: Some(200_000),
            max_output_tokens: Some(64_000),
            reasoning: Some(ReasoningDetails::budgetted(128, Some(24576))),
            knowledge_cutoff: None,
            deprecated: None,
            features: vec![],
        },
        ProviderId::Openai => ModelDetails {
            id: "openai/gpt-5-mini".parse().unwrap(),
            display_name: Some("GPT-5 mini".to_owned()),
            context_window: Some(400_000),
            max_output_tokens: Some(128_000),
            reasoning: Some(ReasoningDetails::budgetted(0, None)),
            knowledge_cutoff: None,
            deprecated: None,
            features: vec![],
        },
        ProviderId::Llamacpp => ModelDetails {
            id: "llamacpp/llama3:latest".parse().unwrap(),
            display_name: None,
            context_window: None,
            max_output_tokens: None,
            reasoning: None,
            knowledge_cutoff: None,
            deprecated: None,
            features: vec![],
        },
        ProviderId::Ollama => ModelDetails {
            id: "ollama/qwen3:8b".parse().unwrap(),
            display_name: None,
            context_window: None,
            max_output_tokens: None,
            reasoning: None,
            knowledge_cutoff: None,
            deprecated: None,
            features: vec![],
        },
        ProviderId::Openrouter => ModelDetails {
            id: "openrouter/openai/gpt-5-mini".parse().unwrap(),
            display_name: None,
            context_window: Some(200_000),
            max_output_tokens: None,
            reasoning: None,
            knowledge_cutoff: None,
            deprecated: None,
            features: vec![],
        },
        ProviderId::Xai => unimplemented!(),
        ProviderId::Deepseek => unimplemented!(),
    }
}
