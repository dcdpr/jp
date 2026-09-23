use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use camino_tempfile::tempdir;
use datetime_literal::datetime;
use futures::{StreamExt as _, stream};
use indexmap::IndexMap;
use jp_config::{
    AppConfig, Config as _,
    assistant::tool_choice::ToolChoice,
    conversation::tool::{PartialToolConfig, ToolConfig},
    model::id::Name,
};
use jp_conversation::{
    Conversation, ConversationId, ConversationStream,
    event::{ChatRequest, InquiryResponse, ToolCallResponse},
};
use jp_inquire::prompt::MockPromptBackend;
use jp_llm::{
    Error as LlmError, EventStream, Provider, StreamError,
    event::{Event, EventPart, FinishReason, ToolCallPart},
    model::ModelDetails,
    query::{ChatQuery, QueryContext, QueryStream, ToolExecution},
};
use jp_mcp::{
    Client,
    server::{BuiltinTool, builtin::BuiltinExecutors, http::connect, result::from_mcp},
};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::FsStorageBackend;
use jp_tool::{InvocationContext, Outcome, Question, ToolDefinition, ToolDocs};
use jp_workspace::Workspace;
use rmcp::model::{CallToolRequestParams, Meta};
use serde_json::{Map, Value, json};
use tokio::time::{Duration, timeout};

use super::{PendingStreamTrim, ToolCoordinator, run_turn_loop};
use crate::{
    access::approvals::ApprovalStore, cmd::query::tool::mcp_executor::TerminalExecutorSource,
    signals::testing::detached_router,
};

struct InquiringTool(Arc<AtomicUsize>);

#[async_trait]
impl BuiltinTool for InquiringTool {
    async fn execute(&self, _: &Value, answers: &IndexMap<String, Value>) -> Outcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        if answers.get("confirm") == Some(&json!(true)) {
            return "confirmed".into();
        }
        Question::boolean("confirm", "Continue?").unwrap().into()
    }
}

struct AgentProvider {
    starts: AtomicUsize,
    storage: Arc<FsStorageBackend>,
    conversation: ConversationId,
    config: AppConfig,
}

#[async_trait]
impl Provider for AgentProvider {
    async fn model_details(&self, _: &Name) -> Result<ModelDetails, LlmError> {
        Ok(ModelDetails::empty("anthropic/test".parse().unwrap()))
    }
    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![])
    }
    async fn chat_completion_stream(
        &self,
        _: &ModelDetails,
        _: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        panic!("the continuation must not become another provider request")
    }
    async fn start_query(
        &self,
        _: &ModelDetails,
        _: ChatQuery,
        context: QueryContext,
    ) -> Result<QueryStream, LlmError> {
        assert_eq!(self.starts.fetch_add(1, Ordering::SeqCst), 0);
        let url = context.mcp_endpoint.unwrap();
        let client = connect(&url).await.unwrap();
        let storage = self.storage.clone();
        let id = self.conversation;
        let config = self.config.clone();
        let stream = async_stream::stream! {
            yield Ok(Event::ToolCallPending { id: "agent-call".into(), name: "http_tool".into() });
            yield Ok(Event::ToolCallPending { id: "agent-call-2".into(), name: "http_tool".into() });
            let stored = serde_json::from_str(&storage.read_test_events_raw(&id).unwrap()).unwrap();
            let events = ConversationStream::from_parts(json!({}), stored, &config.clone().into()).unwrap();
            assert_eq!(events.iter().filter_map(|event| event.event.as_tool_call_request()).count(), 0);
            let mut params = CallToolRequestParams::new("http_tool");
            params.meta = Some(Meta(Map::from_iter([("test/agentId".into(), "agent-call".into())])));
            let peer = client.peer().clone();
            let call = tokio::spawn(async move { peer.call_tool(params).await });
            yield Ok(Event::Part { index: 0, part: EventPart::ToolCall(ToolCallPart::Start { id: "agent-call".into(), name: "http_tool".into() }), metadata: Map::new() });
            yield Ok(Event::Part { index: 0, part: EventPart::ToolCall(ToolCallPart::ArgumentChunk("{}".into())), metadata: Map::new() });
            yield Ok(Event::flush(0));
            yield Ok(Event::Finished(FinishReason::Completed));
            let result = call.await.unwrap().unwrap();
            assert_eq!(from_mcp(result).unwrap().to_text(), "confirmed");
            let mut second = CallToolRequestParams::new("http_tool");
            second.meta = Some(Meta(Map::from_iter([("test/agentId".into(), "agent-call-2".into())])));
            let peer = client.peer().clone();
            let call = tokio::spawn(async move { peer.call_tool(second).await });
            yield Ok(Event::Part { index: 1, part: EventPart::ToolCall(ToolCallPart::Start { id: "agent-call-2".into(), name: "http_tool".into() }), metadata: Map::new() });
            yield Ok(Event::Part { index: 1, part: EventPart::ToolCall(ToolCallPart::ArgumentChunk("{}".into())), metadata: Map::new() });
            yield Ok(Event::flush(1));
            yield Ok(Event::Finished(FinishReason::Completed));
            let result = call.await.unwrap().unwrap();
            assert_eq!(from_mcp(result).unwrap().to_text(), "confirmed");
            let stored = serde_json::from_str(&storage.read_test_events_raw(&id).unwrap()).unwrap();
            let events = ConversationStream::from_parts(json!({}), stored, &config.into()).unwrap();
            let responses = events.iter().filter_map(|event| event.event.as_tool_call_response()).cloned().collect::<Vec<_>>();
            assert_eq!(responses, vec![
                ToolCallResponse { id: "agent-call".into(), result: Ok("confirmed".into()) },
                ToolCallResponse { id: "agent-call-2".into(), result: Ok("confirmed".into()) },
            ]);
            yield Ok(Event::Part { index: 2, part: EventPart::Message("Finished.".into()), metadata: Map::new() });
            yield Ok(Event::flush(2));
            client.cancel().await.unwrap();
            yield Ok(Event::Finished(FinishReason::Completed));
        };
        Ok(QueryStream {
            events: Box::pin(stream),
            execution: ToolExecution::Agent {
                correlation_key: "test/agentId",
            },
        })
    }
}

struct SwitchingAgentProvider {
    first: AgentProvider,
    starts: AtomicUsize,
    next_execution: ToolExecution,
}

#[async_trait]
impl Provider for SwitchingAgentProvider {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails, LlmError> {
        self.first.model_details(name).await
    }
    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![])
    }
    async fn chat_completion_stream(
        &self,
        _: &ModelDetails,
        _: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        panic!("start_query must select the execution contract")
    }
    async fn start_query(
        &self,
        model: &ModelDetails,
        query: ChatQuery,
        context: QueryContext,
    ) -> Result<QueryStream, LlmError> {
        let index = self.starts.fetch_add(1, Ordering::SeqCst);
        if index == 0 {
            let started = self.first.start_query(model, query, context).await?;
            let mut source = started.events;
            let events = async_stream::stream! {
                let mut answered = false;
                while let Some(event) = source.next().await {
                    if matches!(&event, Ok(Event::Part { part: EventPart::Message(_), .. })) { answered = true; }
                    if answered && matches!(&event, Ok(Event::Finished(_))) {
                        yield Err(StreamError::subscription_exhausted("quota reached", None, None).with_credential_change());
                        return;
                    }
                    yield event;
                }
            };
            return Ok(QueryStream {
                events: Box::pin(events),
                execution: started.execution,
            });
        }
        assert_eq!(
            index, 1,
            "credential changes must not restart the original tools"
        );
        let requests = query
            .thread
            .events
            .iter()
            .filter_map(|event| event.event.as_tool_call_request())
            .map(|call| call.id.clone())
            .collect::<Vec<_>>();
        assert_eq!(requests, vec!["agent-call", "agent-call-2"]);
        let responses = query
            .thread
            .events
            .iter()
            .filter_map(|event| event.event.as_tool_call_response())
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(responses, vec![
            ToolCallResponse {
                id: "agent-call".into(),
                result: Ok("confirmed".into())
            },
            ToolCallResponse {
                id: "agent-call-2".into(),
                result: Ok("confirmed".into())
            },
        ]);
        Ok(QueryStream {
            events: Box::pin(stream::iter(vec![
                Ok(Event::Part {
                    index: 0,
                    part: EventPart::Message("Continued.".into()),
                    metadata: Map::new(),
                }),
                Ok(Event::flush(0)),
                Ok(Event::Finished(FinishReason::Completed)),
            ])),
            execution: self.next_execution,
        })
    }
}

#[tokio::test]
async fn agent_quota_fallback_reuses_committed_tools_with_another_agent() {
    assert_agent_fallback(ToolExecution::Agent {
        correlation_key: "test/agentId",
    })
    .await;
}

#[tokio::test]
async fn agent_quota_fallback_can_change_to_caller_owned_execution() {
    assert_agent_fallback(ToolExecution::Caller).await;
}

async fn assert_agent_fallback(next_execution: ToolExecution) {
    timeout(Duration::from_secs(10), async {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let mut config = AppConfig::new_test();
        config.assistant.request.max_retries = 0;
        let partial: PartialToolConfig = serde_json::from_value(json!({"source":"builtin","run":"unattended","style":{"hidden":true},"questions":{"confirm":{"answer":true}}})).unwrap();
        config.conversation.tools.insert("http_tool".into(), ToolConfig::from_partial(partial, vec![]).unwrap());
        let storage = Arc::new(FsStorageBackend::new(&root.join(".jp")).unwrap());
        let mut workspace = Workspace::in_memory(root).with_backend(storage.clone());
        let timestamp = datetime!(2026-09-11 12:00:00 Z);
        let id = ConversationId::try_from(timestamp).unwrap();
        let conversation = Conversation { last_activated_at: timestamp, ..Conversation::default() };
        let lock = workspace.create_and_lock_conversation_with_id(id, conversation, config.clone().into(), None).unwrap();
        let definitions = vec![ToolDefinition { name: "http_tool".into(), docs: ToolDocs::default(), parameters: json!({"type":"object","properties":{}}) }];
        let count = Arc::new(AtomicUsize::new(0));
        let client = Client::default();
        let (source, owner) = TerminalExecutorSource::start(BuiltinExecutors::new().register("http_tool", InquiringTool(count.clone())), &definitions, &config.conversation.tools, Arc::new(ApprovalStore::default()), InvocationContext::default(), &client, root.to_owned()).await.unwrap();
        let provider = Arc::new(SwitchingAgentProvider { first: AgentProvider { starts: AtomicUsize::new(0), storage, conversation: id, config: config.clone() }, starts: AtomicUsize::new(0), next_execution });
        let model = provider.model_details(&"test".parse().unwrap()).await.unwrap();
        let router = detached_router();
        let (printer, output, _) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        run_turn_loop(provider.clone(), &model, &config, &router, root, InvocationContext::default(), false, &[], &lock, ToolChoice::Auto, &definitions, printer.clone(), Arc::new(MockPromptBackend::new()), ToolCoordinator::new(config.conversation.tools.clone(), Box::new(source)), ChatRequest::from("Run the tool."), PendingStreamTrim::default(), router.turn_interrupt(lock.id())).await.unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 4);
        assert_eq!(provider.starts.load(Ordering::SeqCst), 2);
        printer.flush();
        assert_eq!(output.lock().as_str(), "Finished.\n\nContinued.\n\n");
        owner.shutdown().await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn agent_continuation_waits_for_host_recording_without_resubmission() {
    timeout(Duration::from_secs(10), async {
        let temp = tempdir().unwrap();
        let root = temp.path();
        let mut config = AppConfig::new_test();
        let partial: PartialToolConfig = serde_json::from_value(json!({"source":"builtin","run":"unattended","style":{"hidden":true},"questions":{"confirm":{"answer":true}}})).unwrap();
        config.conversation.tools.insert("http_tool".into(), ToolConfig::from_partial(partial, vec![]).unwrap());
        let storage = Arc::new(FsStorageBackend::new(&root.join(".jp")).unwrap());
        let mut workspace = Workspace::in_memory(root).with_backend(storage.clone());
        let timestamp = datetime!(2026-09-11 12:00:00 Z);
        let id = ConversationId::try_from(timestamp).unwrap();
        let conversation = Conversation { last_activated_at: timestamp, ..Conversation::default() };
        let lock = workspace.create_and_lock_conversation_with_id(id, conversation, config.clone().into(), None).unwrap();
        let definitions = vec![ToolDefinition { name: "http_tool".into(), docs: ToolDocs::default(), parameters: json!({"type":"object","properties":{}}) }];
        let count = Arc::new(AtomicUsize::new(0));
        let client = Client::default();
        let (source, owner) = TerminalExecutorSource::start(BuiltinExecutors::new().register("http_tool", InquiringTool(count.clone())), &definitions, &config.conversation.tools, Arc::new(ApprovalStore::default()), InvocationContext::default(), &client, root.to_owned()).await.unwrap();
        let provider = Arc::new(AgentProvider { starts: AtomicUsize::new(0), storage, conversation: id, config: config.clone() });
        let model = provider.model_details(&"test".parse().unwrap()).await.unwrap();
        let router = detached_router();
        let (printer, output, chrome) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        run_turn_loop(provider.clone(), &model, &config, &router, root, InvocationContext::default(), false, &[], &lock, ToolChoice::Auto, &definitions, printer.clone(), Arc::new(MockPromptBackend::new()), ToolCoordinator::new(config.conversation.tools.clone(), Box::new(source)), ChatRequest::from("Run the tool."), PendingStreamTrim::default(), router.turn_interrupt(lock.id())).await.unwrap();
        let answers = lock.events().iter().filter_map(|event| event.event.as_inquiry_response()).filter_map(|answer| match answer { InquiryResponse::Answered { answer, .. } => Some(answer.clone()), _ => None }).collect::<Vec<_>>();
        assert_eq!(answers, vec![json!(true), json!(true)]);
        assert_eq!(count.load(Ordering::SeqCst), 4);
        assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
        printer.flush();
        assert_eq!(output.lock().as_str(), "Finished.\n\n");
        assert_eq!(chrome.lock().as_str(), "\n── \x1b[1mjp\x1b[0m \x1b[2m(anthropic/test)\x1b[0m ─────────────────────────────────────────────────────────\n\n");
        owner.shutdown().await.unwrap();
    }).await.unwrap();
}
