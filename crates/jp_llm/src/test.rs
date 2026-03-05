use std::{panic, sync::Arc};

use chrono::{TimeZone as _, Utc};
use futures::TryStreamExt as _;
use jp_config::{
    AppConfig, PartialAppConfig, ToPartial as _,
    assistant::tool_choice::ToolChoice,
    conversation::tool::ToolParameterConfig,
    model::{
        id::{ModelIdConfig, ModelIdOrAliasConfig, Name, PartialModelIdOrAliasConfig, ProviderId},
        parameters::{
            PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningConfig, ReasoningEffort,
        },
    },
    providers::llm::LlmProviderConfig,
};
use jp_conversation::{
    ConversationEvent, ConversationStream,
    event::{ChatRequest, ToolCallResponse},
    event_builder::EventBuilder,
    stream::ConversationEventWithConfig,
    thread::{Thread, ThreadBuilder},
};
use jp_test::mock::{Snap, Vcr};

use crate::{
    event::Event,
    model::{ModelDetails, ReasoningDetails},
    provider::get_provider,
    query::ChatQuery,
    tool::ToolDefinition,
};

#[allow(clippy::large_enum_variant)]
pub enum TestRequest {
    /// A chat completion request.
    Chat {
        model: ModelDetails,
        query: ChatQuery,
        #[expect(clippy::type_complexity)]
        assert: Arc<dyn Fn(&[Vec<Event>])>,
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
            model: test_model_details(provider),
            query: ChatQuery {
                thread: ThreadBuilder::new()
                    .with_events({
                        let mut config = AppConfig::new_test();
                        config.assistant.model.parameters.reasoning = Some(ReasoningConfig::Off);
                        config.assistant.model.id = ModelIdOrAliasConfig::Id(ModelIdConfig {
                            provider,
                            name: "test".parse().unwrap(),
                        });
                        ConversationStream::new(config.into())
                            .with_created_at(Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap())
                    })
                    .build()
                    .unwrap(),
                tools: vec![],
                tool_choice: ToolChoice::default(),
            },
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

    pub fn model(mut self, model: ModelIdConfig) -> Self {
        let Some(thread) = self.as_thread_mut() else {
            return self;
        };

        let mut delta = PartialAppConfig::empty();
        delta.assistant.model.id = PartialModelIdOrAliasConfig::Id(model.to_partial());
        thread.events.add_config_delta(delta);

        if let Self::Chat { model: m, .. } = &mut self {
            m.id = model;
        }

        self
    }

    pub fn enable_reasoning(self) -> Self {
        self.reasoning(Some(PartialReasoningConfig::Custom(
            PartialCustomReasoningConfig {
                effort: Some(ReasoningEffort::Low),
                exclude: Some(false),
            },
        )))
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

    #[expect(dead_code)]
    pub fn assert_chat(mut self, assert: impl Fn(&[Vec<Event>]) + 'static) -> Self {
        if let Self::Chat { assert: a, .. } = &mut self {
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
            _ => None,
        }
    }

    pub fn as_thread_mut(&mut self) -> Option<&mut Thread> {
        match self {
            Self::Chat { query, .. } => Some(&mut query.thread),
            _ => None,
        }
    }

    pub fn as_model_details_mut(&mut self) -> Option<&mut ModelDetails> {
        match self {
            Self::Chat { model, .. } => Some(model),
            _ => None,
        }
    }
}

impl std::fmt::Debug for TestRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Chat { model, query, .. } => f
                .debug_struct("Chat")
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
                                v.event.timestamp =
                                    Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();
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
                        model,
                        query,
                        assert,
                    } => {
                        let events: Vec<Event> = provider
                            .chat_completion_stream(&model, query)
                            .await
                            .unwrap()
                            .try_collect()
                            .await
                            .unwrap();

                        let stream = conversation_stream
                            .as_mut()
                            .expect("Chat request always sets conversation_stream");
                        let mut builder = EventBuilder::new();

                        for llm_event in events {
                            match llm_event {
                                Event::Part { index: idx, event } => {
                                    builder.handle_part(idx, event);
                                }
                                Event::Flush {
                                    index: idx,
                                    metadata,
                                } => {
                                    if let Some(mut event) = builder.handle_flush(idx, metadata) {
                                        // Normalize timestamp for deterministic snapshots.
                                        event.timestamp =
                                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

                                        all_events[index].push(Event::Part {
                                            index: idx,
                                            event: event.clone(),
                                        });
                                        all_events[index].push(Event::flush(idx));

                                        history.push(ConversationEventWithConfig {
                                            event: event.clone(),
                                            config: config.clone(),
                                        });
                                        stream.push(event);
                                    }
                                }
                                Event::Finished(reason) => {
                                    for mut event in builder.drain() {
                                        event.timestamp =
                                            Utc.with_ymd_and_hms(2020, 1, 1, 0, 0, 0).unwrap();

                                        all_events[index].push(Event::Part {
                                            index: 0,
                                            event: event.clone(),
                                        });
                                        all_events[index].push(Event::flush(0));

                                        history.push(ConversationEventWithConfig {
                                            event: event.clone(),
                                            config: config.clone(),
                                        });
                                        stream.push(event);
                                    }

                                    all_events[index].push(Event::Finished(reason));
                                }
                            }
                        }

                        assert(&all_events);
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
            reasoning: Some(ReasoningDetails::budgetted(512, Some(24576))),
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
        ProviderId::Test => ModelDetails::empty("test/mock-model".parse().unwrap()),
        ProviderId::Xai => unimplemented!(),
        ProviderId::Deepseek => unimplemented!(),
    }
}
