use std::{mem, str::FromStr as _};

use async_stream::stream;
use async_trait::async_trait;
use futures::StreamExt as _;
use jp_config::llm;
use jp_conversation::{
    model::ProviderId,
    thread::{Document, Documents, Thread},
    AssistantMessage, MessagePair, Model, UserMessage,
};
use jp_mcp::tool::ToolChoice;
use jp_query::query::ChatQuery;
use ollama_rs::{
    generation::{
        chat::{request::ChatMessageRequest, ChatMessage, ChatMessageResponse, MessageRole},
        parameters::{KeepAlive, TimeUnit},
        tools::{ToolCall, ToolCallFunction, ToolFunctionInfo, ToolInfo, ToolType},
    },
    models::{LocalModel, ModelOptions},
    Ollama as Client,
};
use serde_json::Value;
use tracing::trace;
use url::Url;

use super::{handle_delta, Event, EventStream, ModelDetails, Provider, Reply, StreamEvent};
use crate::{
    error::{Error, Result},
    provider::{AccumulationState, Delta},
    CompletionChunk,
};

#[derive(Debug, Clone)]
pub struct Ollama {
    client: Client,
}

#[async_trait]
impl Provider for Ollama {
    async fn models(&self) -> Result<Vec<ModelDetails>> {
        let models = self.client.list_local_models().await?;

        Ok(models.into_iter().map(map_model).collect())
    }

    async fn chat_completion(&self, model: &Model, query: ChatQuery) -> Result<Reply> {
        let request = create_request(model, query)?;
        self.client
            .send_chat_messages(request)
            .await
            .map_err(Into::into)
            .and_then(map_response)
            .map(Reply)
    }

    async fn chat_completion_stream(&self, model: &Model, query: ChatQuery) -> Result<EventStream> {
        let client = self.client.clone();
        let request = create_request(model, query)?;
        let stream = Box::pin(stream! {
            let mut current_state = AccumulationState::default();
            let mut extractor = ReasoningExtractor::default();

            let stream = client
                .send_chat_messages_stream(request.clone()).await
                .map_err(Error::from)?;

            tokio::pin!(stream);
            while let Some(event) = stream.next().await {
                let events = event
                    .map(|event| {
                        extractor.handle(&event.message.content);
                        map_event(event, &mut current_state, &mut extractor)
                    })
                    .unwrap_or_default();

                for event in events {
                    yield event;
                }
            }

            extractor.finalize();
            for event in map_content(&mut current_state, &mut extractor) {
                yield event;
            }
        });

        Ok(stream)
    }
}

fn map_model(model: LocalModel) -> ModelDetails {
    ModelDetails {
        provider: ProviderId::Ollama,
        slug: model.name,
        context_window: None,
        max_output_tokens: None,
        reasoning: None,
        knowledge_cutoff: None,
    }
}

fn map_response(response: ChatMessageResponse) -> Result<Vec<Event>> {
    let mut extractor = ReasoningExtractor::default();
    extractor.handle(&response.message.content);
    extractor.finalize();

    map_event(response, &mut AccumulationState::default(), &mut extractor)
        .into_iter()
        .map(|v| {
            v.map(|e| match e {
                StreamEvent::ChatChunk(content) => match content {
                    CompletionChunk::Content(s) => Event::Content(s),
                    CompletionChunk::Reasoning(s) => Event::Reasoning(s),
                },
                StreamEvent::ToolCall(request) => Event::ToolCall(request),
                StreamEvent::Metadata(key, metadata) => Event::Metadata(key, metadata),
            })
        })
        .collect::<Result<Vec<_>>>()
}

fn map_event(
    event: ChatMessageResponse,
    state: &mut AccumulationState,
    extractor: &mut ReasoningExtractor,
) -> Vec<Result<StreamEvent>> {
    let mut events = vec![];

    for tool_call in event.message.tool_calls {
        let delta = Delta::tool_call(
            "",
            &tool_call.function.name,
            tool_call.function.arguments.to_string(),
        )
        .finished();

        events.extend(handle_delta(delta, state).transpose());
    }

    events.extend(map_content(state, extractor));
    events
}

fn map_content(
    state: &mut AccumulationState,
    extractor: &mut ReasoningExtractor,
) -> Vec<Result<StreamEvent>> {
    let mut events = Vec::new();
    if !extractor.reasoning.is_empty() {
        let reasoning = mem::take(&mut extractor.reasoning);
        events.extend(handle_delta(Delta::reasoning(reasoning), state).transpose());
    }

    if !extractor.other.is_empty() {
        let content = mem::take(&mut extractor.other);
        events.extend(handle_delta(Delta::content(content), state).transpose());
    }

    events
}

fn create_request(model: &Model, query: ChatQuery) -> Result<ChatMessageRequest> {
    let ChatQuery {
        thread,
        tools,
        tool_choice,
        tool_call_strict_mode,
    } = query;

    let mut request = ChatMessageRequest::new(
        model.id.slug().to_owned(),
        convert_thread(thread, tool_choice)?,
    );

    let tools = convert_tools(tools, tool_call_strict_mode)?;
    if !tools.is_empty() {
        request = request.tools(tools);
    }

    let mut options = ModelOptions::default();

    if let Some(temperature) = model.parameters.temperature {
        options = options.temperature(temperature);
    }

    if let Some(top_p) = model.parameters.top_p {
        options = options.top_p(top_p);
    }

    if let Some(top_k) = model.parameters.top_k {
        options = options.top_k(top_k);
    }

    // Set the context window for the model.
    //
    // This can be used to force Ollama to use a larger context window then the
    // one determined based on the machine's resources.
    if let Some(context_window) = model
        .parameters
        .other
        .get("context_window")
        .and_then(Value::as_u64)
    {
        options = options.num_ctx(context_window);
    }

    if let Some(keep_alive) = model
        .parameters
        .other
        .get("keep_alive")
        .and_then(Value::as_str)
    {
        let unit = keep_alive
            .chars()
            .last()
            .filter(char::is_ascii_alphabetic)
            .unwrap_or('m');

        let value = keep_alive
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect::<String>();

        request = request.keep_alive(KeepAlive::Until {
            time: value.parse::<u64>().unwrap_or(5),
            unit: match unit {
                's' => TimeUnit::Seconds,
                'h' => TimeUnit::Hours,
                _ => TimeUnit::Minutes,
            },
        });
    }

    request = request.options(options);

    Ok(request)
}

impl TryFrom<&llm::provider::ollama::Config> for Ollama {
    type Error = Error;

    fn try_from(config: &llm::provider::ollama::Config) -> Result<Self> {
        let url = Url::from_str(&config.base_url)?;
        let port = url.port().unwrap_or(11434);
        let client = reqwest::Client::new();

        Ok(Ollama {
            client: Client::new_with_client(url, port, client),
        })
    }
}

fn convert_tools(tools: Vec<jp_mcp::Tool>, _strict: bool) -> Result<Vec<ToolInfo>> {
    tools
        .into_iter()
        .map(|tool| {
            Ok(ToolInfo {
                tool_type: ToolType::Function,
                function: ToolFunctionInfo {
                    name: tool.name.to_string(),
                    description: tool.description.unwrap_or_default().to_string(),
                    parameters: serde_json::from_value(serde_json::Value::Object(
                        tool.input_schema.as_ref().clone(),
                    ))?,
                },
            })
        })
        .collect::<Result<Vec<_>>>()
}
//
fn convert_thread(thread: Thread, tool_choice: ToolChoice) -> Result<Vec<ChatMessage>> {
    Messages::try_from((thread, tool_choice)).map(|v| v.0)
}
struct Messages(Vec<ChatMessage>);

impl TryFrom<(Thread, ToolChoice)> for Messages {
    type Error = Error;

    #[expect(clippy::too_many_lines)]
    fn try_from((thread, tool_choice): (Thread, ToolChoice)) -> Result<Self> {
        let Thread {
            system_prompt,
            instructions,
            attachments,
            mut history,
            message,
        } = thread;

        // If the last history message is a tool call response, we need to go
        // one more back in history, to avoid disjointing tool call requests and
        // their responses.
        let mut history_after_instructions = vec![];
        while let Some(message) = history.pop() {
            let tool_call_results = matches!(message.message, UserMessage::ToolCallResults(_));
            history_after_instructions.insert(0, message);

            if !tool_call_results {
                break;
            }
        }

        let mut items = vec![];
        let history = history
            .into_iter()
            .flat_map(message_pair_to_messages)
            .collect::<Vec<_>>();

        // System message first, if any.
        if let Some(system_prompt) = system_prompt {
            items.push(ChatMessage {
                role: MessageRole::System,
                content: system_prompt,
                tool_calls: vec![],
                images: None,
            });
        }

        // Historical messages second, these are static.
        items.extend(history);

        // Group multiple contents blocks into a single message.
        let mut content = vec![];

        if !instructions.is_empty() {
            // TODO: Poor-man's version of API-based tool choice. Needed until
            // Ollama has first-class support for tool choice.
            match tool_choice {
                // For `auto`, we leave it up to the model to decide.
                // For `none`, we already remove all tools from the request, so
                // there is no need to instruct the model any further.
                ToolChoice::Auto | ToolChoice::None => {}
                ToolChoice::Required => {
                    content.push(
                        // sigh.. Shouting seems to be a tad bit more effective.
                        "IMPORTANT: You MUST use one or more functions or tools available to you. \
                         DO NOT QUESTION THIS DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR \
                         DETAILS. JUST RUN IT."
                            .to_string(),
                    );
                }
                ToolChoice::Function(name) => {
                    content.push(format!(
                        "IMPORTANT: You MUST use the function or tool named '{name}' available to \
                         you. DO NOT QUESTION THIS DIRECTIVE. DO NOT PROMPT FOR MORE CONTEXT OR \
                         DETAILS. JUST RUN IT."
                    ));
                }
            }

            content.push(
                "Before we continue, here are some contextual details that will help you generate \
                 a better response."
                    .to_string(),
            );
        }

        // Then instructions in XML tags.
        for instruction in &instructions {
            content.push(instruction.try_to_xml()?);
        }

        // Then large list of attachments, formatted as XML.
        if !attachments.is_empty() {
            let documents: Documents = attachments
                .into_iter()
                .enumerate()
                .inspect(|(i, attachment)| trace!("Attaching {}: {}", i, attachment.source))
                .map(Document::from)
                .collect::<Vec<_>>()
                .into();

            content.push(documents.try_to_xml()?);
        }

        // Attach all data, and add a "fake" acknowledgement by the assistant.
        //
        // See `provider::openrouter` for more information.
        if !content.is_empty() {
            items.push(ChatMessage {
                role: MessageRole::User,
                content: content.join("\n\n"),
                tool_calls: vec![],
                images: None,
            });
        }

        if items
            .last()
            .is_some_and(|m| matches!(m.role, MessageRole::User))
        {
            items.push(ChatMessage {
                role: MessageRole::Assistant,
                content: "Thank you for those details, I'll use them to inform my next response."
                    .into(),
                tool_calls: vec![],
                images: None,
            });
        }

        items.extend(
            history_after_instructions
                .into_iter()
                .flat_map(message_pair_to_messages),
        );

        // User query
        match message {
            UserMessage::Query(text) => {
                items.push(ChatMessage {
                    role: MessageRole::User,
                    content: text,
                    tool_calls: vec![],
                    images: None,
                });
            }
            UserMessage::ToolCallResults(results) => {
                items.extend(results.into_iter().map(|result| ChatMessage {
                    role: MessageRole::Tool,
                    content: result.content,
                    tool_calls: vec![],
                    images: None,
                }));
            }
        }

        Ok(Self(items))
    }
}
fn message_pair_to_messages(msg: MessagePair) -> Vec<ChatMessage> {
    let (user, assistant) = msg.split();

    user_message_to_messages(user)
        .into_iter()
        .chain(assistant_message_to_messages(assistant))
        .collect()
}

fn user_message_to_messages(user: UserMessage) -> Vec<ChatMessage> {
    match user {
        UserMessage::Query(query) if !query.is_empty() => vec![ChatMessage {
            role: MessageRole::User,
            content: query,
            tool_calls: vec![],
            images: None,
        }],
        UserMessage::Query(_) => vec![],
        UserMessage::ToolCallResults(results) => results
            .into_iter()
            .map(|result| ChatMessage {
                role: MessageRole::Tool,
                content: result.content,
                tool_calls: vec![],
                images: None,
            })
            .collect(),
    }
}

fn assistant_message_to_messages(assistant: AssistantMessage) -> Vec<ChatMessage> {
    let AssistantMessage {
        content,
        tool_calls,
        ..
    } = assistant;

    let mut items = vec![];
    if let Some(text) = content {
        items.push(ChatMessage {
            role: MessageRole::Assistant,
            content: text,
            tool_calls: vec![],
            images: None,
        });
    }

    items.push(ChatMessage {
        role: MessageRole::Assistant,
        content: String::new(),
        tool_calls: tool_calls
            .into_iter()
            .map(|call| ToolCall {
                function: ToolCallFunction {
                    name: call.name,
                    arguments: call.arguments,
                },
            })
            .collect(),
        images: None,
    });

    items
}

impl From<ToolCall> for Delta {
    fn from(item: ToolCall) -> Self {
        Delta::tool_call(
            &item.function.name,
            &item.function.name,
            item.function.arguments.to_string(),
        )
    }
}

#[derive(Default, Debug)]
/// A parser that segments a stream of text into 'reasoning' and 'other' buckets.
/// It handles streams with or without a `<think>` block.
pub struct ReasoningExtractor {
    pub other: String,
    pub reasoning: String,
    buffer: String,
    state: ReasoningState,
}

#[derive(Default, PartialEq, Debug)]
enum ReasoningState {
    #[default]
    /// The default state. Processing 'other' text while looking for `<think>\n`.
    Idle,
    /// Found `<think>\n`. Processing 'reasoning' text while looking for `</think>\n`.
    Accumulating,
    /// Found `</think>\n`. All subsequent text is 'other'.
    Finished,
}

impl ReasoningExtractor {
    /// Processes a chunk of the incoming text stream.
    pub fn handle(&mut self, text: &str) {
        self.buffer.push_str(text);

        loop {
            match self.state {
                ReasoningState::Idle => {
                    if let Some(tag_start_index) = self.buffer.find("<think>\n") {
                        // Tag found. Text before it is 'other'.
                        self.other.push_str(&self.buffer[..tag_start_index]);

                        // Drain the processed 'other' text and the tag itself.
                        let tag_end_offset = tag_start_index + "<think>\n".len();
                        self.buffer.drain(..tag_end_offset);

                        // Transition state and re-process the rest of the buffer.
                        self.state = ReasoningState::Accumulating;
                    } else {
                        // No tag found. We can safely move most of the buffer to `other`,
                        // but must keep a small tail in case a tag is split across chunks.
                        let tail_len = self.buffer.len().min("<think>\n".len() - 1);
                        let drain_to = self.buffer.len() - tail_len;

                        if drain_to > 0 {
                            self.other.push_str(&self.buffer[..drain_to]);
                            self.buffer.drain(..drain_to);
                        }

                        // Wait for more data.
                        return;
                    }
                }
                ReasoningState::Accumulating => {
                    if let Some(tag_start_index) = self.buffer.find("</think>\n") {
                        // Closing tag found. Text before it is 'thinking'.
                        self.reasoning.push_str(&self.buffer[..tag_start_index]);

                        // Drain the 'reasoning' text and the tag.
                        let tag_end_offset = tag_start_index + "</think>\n".len();
                        self.buffer.drain(..tag_end_offset);

                        // Transition state and re-process.
                        self.state = ReasoningState::Finished;
                    } else {
                        // No closing tag found yet. Move "safe" part of the buffer to `reasoning`.
                        let tail_len = self.buffer.len().min("</think>\n".len() - 1);
                        let drain_to = self.buffer.len() - tail_len;

                        if drain_to > 0 {
                            self.reasoning.push_str(&self.buffer[..drain_to]);
                            self.buffer.drain(..drain_to);
                        }

                        // Wait for more data.
                        return;
                    }
                }
                ReasoningState::Finished => {
                    // Everything from now on is 'other'. No need for complex buffering.
                    self.other.push_str(&self.buffer);
                    self.buffer.clear();
                    return;
                }
            }
        }
    }

    /// Call this after the stream has finished to process any remaining data.
    pub fn finalize(&mut self) {
        match self.state {
            ReasoningState::Accumulating => {
                let (reasoning, other) = self
                    .buffer
                    .split_once("</think>")
                    .unwrap_or((self.buffer.as_str(), ""));

                self.reasoning.push_str(reasoning);
                self.other.push_str(other);
            }
            _ => {
                self.other.push_str(&self.buffer);
            }
        }
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, result::Result};

    use jp_conversation::ModelId;
    use jp_query::structured::conversation_titles;
    use jp_test::{function_name, mock::Vcr};
    use test_log::test;

    use super::*;

    fn vcr(url: &str) -> Vcr {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        Vcr::new(url, fixtures)
    }

    #[test(tokio::test)]
    async fn test_ollama_models() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;

                Ollama::try_from(&config)
                    .unwrap()
                    .models()
                    .await
                    .map(|mut v| {
                        v.truncate(2);
                        v
                    })
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_ollama_chat_completion() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let model: ModelId = "ollama/llama3:latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;

                Ollama::try_from(&config)
                    .unwrap()
                    .chat_completion(&model.into(), query)
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_ollama_chat_completion_stream() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let model: ModelId = "ollama/llama3:latest".parse().unwrap();
        let query = ChatQuery {
            thread: Thread {
                message: "Test message".into(),
                ..Default::default()
            },
            ..Default::default()
        };

        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;

                Ollama::try_from(&config)
                    .unwrap()
                    .chat_completion_stream(&model.into(), query)
                    .await
                    .unwrap()
                    .filter_map(
                        |r| async move { r.unwrap().into_chat_chunk().unwrap().into_content() },
                    )
                    .collect::<String>()
                    .await
            },
        )
        .await
    }

    #[test(tokio::test)]
    async fn test_ollama_structured_completion() -> Result<(), Box<dyn std::error::Error>> {
        let mut config = llm::Config::default().provider.ollama;
        let model: ModelId = "ollama/llama3.1:8b".parse().unwrap();

        let message = UserMessage::Query("Test message".to_string());
        let history = vec![MessagePair::new(message, AssistantMessage::default())];

        let vcr = vcr(&config.base_url);
        vcr.cassette(
            function_name!(),
            |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            },
            |_, url| async move {
                config.base_url = url;
                let query = conversation_titles(3, history, &[]).unwrap();

                Ollama::try_from(&config)
                    .unwrap()
                    .structured_completion(&model.into(), query)
                    .await
            },
        )
        .await
    }

    mod chunk_parser {
        use test_log::test;

        use super::*;

        #[test]
        fn test_no_think_tag_at_all() {
            let mut parser = ReasoningExtractor::default();
            parser.handle("some other text");
            parser.finalize();
            assert_eq!(parser.other, "some other text");
            assert_eq!(parser.reasoning, "");
        }

        #[test]
        fn test_standard_case_with_newline() {
            let mut parser = ReasoningExtractor::default();
            parser.handle("prefix\n<think>\nthoughts\n</think>\nsuffix");
            parser.finalize();
            assert_eq!(parser.reasoning, "thoughts\n");
            assert_eq!(parser.other, "prefix\nsuffix");
        }

        #[test]
        fn test_suffix_only() {
            let mut parser = ReasoningExtractor::default();
            parser.handle("<think>\nthoughts\n</think>\n\nsuffix text here");
            parser.finalize();
            assert_eq!(parser.reasoning, "thoughts\n");
            assert_eq!(parser.other, "\nsuffix text here");
        }

        #[test]
        fn test_ends_with_closing_tag_no_newline() {
            let mut parser = ReasoningExtractor::default();
            parser.handle("<think>\nfinal thoughts\n");
            parser.handle("</think>");
            parser.finalize();
            assert_eq!(parser.reasoning, "final thoughts\n");
            assert_eq!(parser.other, "");
        }

        #[test]
        fn test_less_than_symbol_in_reasoning_content_is_not_stripped() {
            let mut parser = ReasoningExtractor::default();
            parser.handle("<think>\na < b is a true statement\n</think>");
            parser.finalize();
            // The last '<' is part of "</think>", so "a < b is a true statement" is kept.
            assert_eq!(parser.reasoning, "a < b is a true statement\n");
        }

        #[test]
        fn test_less_than_symbol_not_part_of_tag_is_kept() {
            let mut parser = ReasoningExtractor::default();
            parser.handle("<think>\nhere is a random < symbol");
            parser.finalize();
            // The final '<' is not a prefix of '</think>', so it's kept.
            assert_eq!(parser.reasoning, "here is a random < symbol");
        }
    }
}
