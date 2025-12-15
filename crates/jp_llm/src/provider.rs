// pub mod deepseek;
pub mod google;
// pub mod xai;
pub mod anthropic;
pub mod llamacpp;
pub mod ollama;
pub mod openai;
pub mod openrouter;

use anthropic::Anthropic;
use async_trait::async_trait;
use futures::{TryStreamExt as _, stream};
use google::Google;
use jp_config::{
    assistant::instructions::InstructionsConfig,
    model::id::{Name, ProviderId},
    providers::llm::LlmProviderConfig,
};
use jp_conversation::event::ConversationEvent;
use llamacpp::Llamacpp;
use ollama::Ollama;
use openai::Openai;
use openrouter::Openrouter;
use serde_json::Value;
use tracing::warn;

use crate::{
    Error,
    error::Result,
    event::Event,
    model::ModelDetails,
    query::{ChatQuery, StructuredQuery},
    stream::{EventStream, aggregator::chunk::EventAggregator},
    structured::SCHEMA_TOOL_NAME,
};

#[async_trait]
pub trait Provider: std::fmt::Debug + Send + Sync {
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

    /// Perform a non-streaming chat completion.
    ///
    /// Default implementation collects results from the streaming version.
    async fn chat_completion(&self, model: &ModelDetails, query: ChatQuery) -> Result<Vec<Event>> {
        let mut aggregator = EventAggregator::new();
        self.chat_completion_stream(model, query)
            .await?
            .map_ok(|event| stream::iter(aggregator.ingest(event).into_iter().map(Ok)))
            .try_flatten()
            .try_collect()
            .await
    }

    /// Perform a structured completion.
    ///
    /// Default implementation uses a specialized tool-call to get structured
    /// results.
    ///
    /// Providers that have a dedicated structured response endpoint should
    /// override this method.
    async fn structured_completion(
        &self,
        model: &ModelDetails,
        query: StructuredQuery,
    ) -> Result<Value> {
        let mut chat_query = ChatQuery {
            thread: query.thread.clone(),
            tools: vec![query.tool_definition()?],
            tool_choice: query.tool_choice()?,
            tool_call_strict_mode: true,
        };

        let max_retries = 3;
        for i in 1..=max_retries {
            let result = self.chat_completion(model, chat_query.clone()).await;
            let events = match result {
                Ok(events) => events,
                Err(error) if i >= max_retries => return Err(error),
                Err(error) => {
                    warn!(%error, "Error while getting structured data. Retrying in non-strict mode.");
                    chat_query.tool_call_strict_mode = false;
                    continue;
                }
            };

            let data = events
                .into_iter()
                .filter_map(Event::into_conversation_event)
                .filter_map(ConversationEvent::into_tool_call_request)
                .find(|call| call.name == SCHEMA_TOOL_NAME)
                .map(|call| Value::Object(call.arguments));

            let result = data
                .ok_or("Did not receive any structured data".to_owned())
                .and_then(|data| query.validate(&data).map(|()| data));

            match result {
                Ok(data) => return Ok(query.map(data)),
                Err(error) => {
                    warn!(error, "Failed to fetch structured data. Retrying.");

                    chat_query.thread.instructions.push(
                        InstructionsConfig::default()
                            .with_title("Structured Data Validation Error")
                            .with_description(
                                "The following error occurred while validating the structured \
                                 data. Please try again.",
                            )
                            .with_item(error),
                    );
                }
            }
        }

        Err(Error::MissingStructuredData)
    }
}

/// Get a provider by ID.
///
/// # Panics
///
/// Panics if the provider is `ProviderId::TEST`, which is reserved for testing
/// only.
pub fn get_provider(id: ProviderId, config: &LlmProviderConfig) -> Result<Box<dyn Provider>> {
    let provider: Box<dyn Provider> = match id {
        ProviderId::Anthropic => Box::new(Anthropic::try_from(&config.anthropic)?),
        ProviderId::Deepseek => todo!(),
        ProviderId::Google => Box::new(Google::try_from(&config.google)?),
        ProviderId::Llamacpp => Box::new(Llamacpp::try_from(&config.llamacpp)?),
        ProviderId::Ollama => Box::new(Ollama::try_from(&config.ollama)?),
        ProviderId::Openai => Box::new(Openai::try_from(&config.openai)?),
        ProviderId::Openrouter => Box::new(Openrouter::try_from(&config.openrouter)?),
        ProviderId::Xai => todo!(),
    };

    Ok(provider)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use jp_config::{
        assistant::tool_choice::ToolChoice,
        conversation::tool::{OneOrManyTypes, ToolParameterConfig, item::ToolParameterItemConfig},
    };
    use jp_conversation::event::ChatRequest;
    use jp_test::{Result, function_name};

    use super::*;
    use crate::{
        structured,
        test::{TestRequest, run_test, test_model_details},
    };

    macro_rules! test_all_providers {
        ($($fn:ident),* $(,)?) => {
            mod anthropic { use super::*; $(test_all_providers!(func; $fn, ProviderId::Anthropic);)* }
            mod google    { use super::*; $(test_all_providers!(func; $fn, ProviderId::Google);)* }
            mod openai    { use super::*; $(test_all_providers!(func; $fn, ProviderId::Openai);)* }
            mod openrouter{ use super::*; $(test_all_providers!(func; $fn, ProviderId::Openrouter);)* }
            mod ollama    { use super::*; $(test_all_providers!(func; $fn, ProviderId::Ollama);)* }
            mod llamacpp  { use super::*; $(test_all_providers!(func; $fn, ProviderId::Llamacpp);)* }
        };
        (func; $fn:ident, $provider:ty) => {
            paste::paste! {
                #[test_log::test(tokio::test)]
                async fn [< test_ $fn >]() -> Result {
                    $fn($provider, function_name!()).await
                }
            }
        };
    }

    async fn chat_completion_nostream(provider: ProviderId, test_name: &str) -> Result {
        let request = TestRequest::chat(provider)
            .stream(false)
            .enable_reasoning()
            .event(ChatRequest::from("Test message"));

        run_test(provider, test_name, Some(request)).await
    }

    async fn chat_completion_stream(provider: ProviderId, test_name: &str) -> Result {
        let request = TestRequest::chat(provider)
            .stream(true)
            .enable_reasoning()
            .event(ChatRequest::from("Test message"));

        run_test(provider, test_name, Some(request)).await
    }

    fn tool_call_base(provider: ProviderId) -> TestRequest {
        TestRequest::chat(provider)
            .event(ChatRequest::from("Testing tool call"))
            .tool("run_me", vec![
                ("foo", ToolParameterConfig {
                    kind: OneOrManyTypes::One("string".into()),
                    default: Some("foo".into()),
                    description: None,
                    required: false,
                    enumeration: vec![],
                    items: None,
                }),
                ("bar", ToolParameterConfig {
                    kind: OneOrManyTypes::Many(vec!["string".into(), "array".into()]),
                    default: None,
                    description: None,
                    required: true,
                    enumeration: vec!["foo".into(), vec!["foo", "bar"].into()],
                    items: Some(ToolParameterItemConfig {
                        kind: OneOrManyTypes::One("string".into()),
                        default: None,
                        description: None,
                        enumeration: vec![],
                    }),
                }),
            ])
    }

    async fn tool_call_nostream(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider),
            TestRequest::tool_call_response(Ok("working!"), false),
        ];

        run_test(provider, test_name, requests).await
    }

    async fn tool_call_stream(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider).stream(true),
            TestRequest::tool_call_response(Ok("working!"), false),
        ];

        run_test(provider, test_name, requests).await
    }

    async fn tool_call_strict(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider).tool_call_strict_mode(true),
            TestRequest::tool_call_response(Ok("working!"), false),
        ];

        run_test(provider, test_name, requests).await
    }

    /// Without reasoning, "forced" tool calls should work as expected.
    async fn tool_call_required_no_reasoning(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider).tool_choice(ToolChoice::Required),
            TestRequest::tool_call_response(Ok("working!"), true),
        ];

        run_test(provider, test_name, requests).await
    }

    /// With reasoning, some models do not support "forced" tool calls, so
    /// provider implementations should fall back to trying to instruct the
    /// model to use the tool through regular textual instructions.
    async fn tool_call_required_reasoning(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider)
                .tool_choice(ToolChoice::Required)
                .enable_reasoning(),
            TestRequest::tool_call_response(Ok("working!"), false),
        ];

        run_test(provider, test_name, requests).await
    }

    async fn tool_call_auto(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider).tool_choice(ToolChoice::Auto),
            TestRequest::tool_call_response(Ok("working!"), false),
        ];

        run_test(provider, test_name, requests).await
    }

    async fn tool_call_function(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider).tool_choice_fn("run_me"),
            TestRequest::tool_call_response(Ok("working!"), true),
        ];

        run_test(provider, test_name, requests).await
    }

    async fn tool_call_reasoning(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            tool_call_base(provider).enable_reasoning(),
            TestRequest::tool_call_response(Ok("working!"), false),
        ];

        run_test(provider, test_name, requests).await
    }

    async fn structured_completion_success(provider: ProviderId, test_name: &str) -> Result {
        let request =
            TestRequest::chat(provider).chat_request("I am testing the structured completion API.");
        let history = request.as_thread().unwrap().events.clone();
        let request = TestRequest::Structured {
            query: structured::titles::titles(3, history, &[]).unwrap(),
            model: match request {
                TestRequest::Chat { model, .. } => model,
                _ => unreachable!(),
            },
            assert: Arc::new(|_| {}),
        };

        run_test(provider, test_name, Some(request)).await
    }

    async fn structured_completion_error(provider: ProviderId, test_name: &str) -> Result {
        let request =
            TestRequest::chat(provider).chat_request("I am testing the structured completion API.");
        let thread = request.as_thread().cloned().unwrap();
        let query = StructuredQuery::new(
            schemars::json_schema!({
                "type": "object",
                "description": "1 + 1 = ?",
                "required": ["answer"],
                "additionalProperties": false,
                "properties": { "answer": { "type": "integer" } },
            }),
            thread,
        )
        .with_validator(move |value| {
            value
                .get("answer")
                .ok_or("Missing `answer` field.".to_owned())?
                .as_u64()
                .ok_or("Answer must be an integer".to_owned())
                .and_then(|v| Err(format!("You thought 1 + 1 = {v}? Think again!")))
        });

        let request = TestRequest::Structured {
            query,
            model: match request {
                TestRequest::Chat { model, .. } => model,
                _ => unreachable!(),
            },
            assert: Arc::new(|results| {
                results.iter().all(std::result::Result::is_err);
            }),
        };

        run_test(provider, test_name, Some(request)).await
    }

    async fn model_details(provider: ProviderId, test_name: &str) -> Result {
        let request = TestRequest::ModelDetails {
            name: test_model_details(provider).id.name.to_string(),
            assert: Arc::new(|_| {}),
        };

        run_test(provider, test_name, Some(request)).await
    }

    async fn models(provider: ProviderId, test_name: &str) -> Result {
        let request = TestRequest::Models {
            assert: Arc::new(|_| {}),
        };

        run_test(provider, test_name, Some(request)).await
    }

    async fn multi_turn_conversation(provider: ProviderId, test_name: &str) -> Result {
        let requests = vec![
            TestRequest::chat(provider).chat_request("Test message"),
            TestRequest::chat(provider)
                .enable_reasoning()
                .chat_request("Repeat my previous message"),
            tool_call_base(provider).tool_choice_fn("run_me"),
            TestRequest::tool_call_response(Ok("The secret code is: 42"), true),
            TestRequest::chat(provider)
                .enable_reasoning()
                .chat_request("What was the result of the previous tool call?"),
        ];

        run_test(provider, test_name, requests).await
    }

    test_all_providers![
        chat_completion_nostream,
        chat_completion_stream,
        tool_call_auto,
        tool_call_function,
        tool_call_reasoning,
        tool_call_nostream,
        tool_call_required_no_reasoning,
        tool_call_required_reasoning,
        tool_call_stream,
        tool_call_strict,
        structured_completion_success,
        structured_completion_error,
        model_details,
        models,
        multi_turn_conversation,
    ];
}
