//! MCP Host adapter for tool calls through JP's loopback HTTP endpoint.
//!
//! The coordinator resolves interactions; this adapter holds their single-use
//! replies across preparation, execution, and final conversation recording.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex as SyncMutex, MutexGuard, PoisonError},
};

use async_trait::async_trait;
use camino::{Utf8Path, Utf8PathBuf};
use futures::future::BoxFuture;
use indexmap::IndexMap;
use jp_config::conversation::tool::{RunMode, ToolConfigWithDefaults, ToolsConfig};
use jp_conversation::event::{InquirySource, ToolCallRequest, ToolCallResponse};
use jp_llm::tool::{Executor, ExecutorResult, ExecutorSource, PermissionInfo};
use jp_mcp::{
    CallToolResult, Client,
    server::{
        InvocationContext, StderrSink,
        builtin::BuiltinExecutors,
        http::{Endpoint, EndpointError},
        service::{
            Admission, ConfiguredTool, HostReply, HostRequest, InputAnswer, Interaction,
            InvocationId, ReleaseDecision, Service,
        },
        text_result,
    },
};
use jp_tool::{AnswerType, ContentBlock, InputRequest, Question, ToolDefinition};
use rand::random;
use rmcp::{
    Peer, ServiceError as McpCallError,
    model::{CallToolRequestParams, Meta},
    service::{RoleClient, RunningService},
};
use serde_json::{Map, Value};
use tokio::{
    sync::{Mutex, broadcast::error::RecvError as ProgressError, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use crate::access::{approvals::ApprovalStore, compile::compile_tool_policy};

const CORRELATION_KEY: &str = "computer.jp/hostCall";

type TextResult = Result<String, String>;
type Reply<T> = oneshot::Sender<HostReply<T>>;
type Calls = Arc<SyncMutex<HashMap<String, Arc<Mutex<PendingCall>>>>>;

struct Route {
    request: ToolCallRequest,
    invocation: Option<InvocationId>,
    sender: mpsc::Sender<HostRequest>,
}

type Routes = Arc<SyncMutex<HashMap<String, Route>>>;
type Sinks = Arc<SyncMutex<HashMap<InvocationId, StderrSink>>>;

fn locked<T>(value: &SyncMutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Creates MCP-backed executors; configuration and execution context are fixed
/// by the MCP Host when starting the endpoint.
pub struct TerminalExecutorSource {
    peer: Peer<RoleClient>,
    service: Arc<Service>,
    definitions: IndexMap<String, ToolDefinition>,
    calls: Calls,
    routes: Routes,
    sinks: Sinks,
}

/// Keeps the listener, MCP connection, and Host routing tasks alive for a turn.
pub struct ExecutionOwner {
    endpoint: Option<Endpoint>,
    client: Option<RunningService<RoleClient, ()>>,
    router: JoinHandle<()>,
    progress: JoinHandle<()>,
}

impl ExecutionOwner {
    /// Cancel pending work and wait for listener/connection cleanup.
    pub async fn shutdown(mut self) -> Result<(), EndpointError> {
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.shutdown().await?;
        }
        if let Some(client) = self.client.take() {
            client.cancel().await?;
        }
        self.router.abort();
        self.progress.abort();
        Ok(())
    }
}

impl Drop for ExecutionOwner {
    fn drop(&mut self) {
        self.router.abort();
        self.progress.abort();
    }
}

impl TerminalExecutorSource {
    /// Start the common MCP execution path and its private Host connection.
    pub async fn start(
        builtins: BuiltinExecutors,
        definitions: &[ToolDefinition],
        tools: &ToolsConfig,
        approvals: Arc<ApprovalStore>,
        invocation: InvocationContext,
        upstream: &Client,
        root: Utf8PathBuf,
    ) -> Result<(Self, ExecutionOwner), EndpointError> {
        let configured = definitions
            .iter()
            .filter_map(|definition| {
                let config = tools.get(&definition.name)?;
                let access =
                    compile_tool_policy(config.access(), &root, &approvals).map_err(|error| {
                        format!(
                            "invalid access policy for tool '{}': {error}",
                            definition.name
                        )
                    });
                Some(ConfiguredTool {
                    definition: definition.clone(),
                    config,
                    access,
                    metadata: Map::new(),
                })
            })
            .collect();
        let (service, mut host) =
            Service::new(configured, upstream.clone(), builtins, root, invocation)?;
        let mut stderr = service.subscribe_progress();
        let endpoint = Endpoint::start(service).await?;
        let client = endpoint.connect().await?;
        let routes = Routes::default();
        let router_routes = routes.clone();
        let router = tokio::spawn(async move {
            while let Some(request) = host.recv().await {
                let sender = {
                    let key = request
                        .call
                        .request
                        .correlation
                        .get(CORRELATION_KEY)
                        .and_then(Value::as_str);
                    let mut routes = locked(&router_routes);
                    key.and_then(|key| routes.get_mut(key)).and_then(|route| {
                        // Correlation associates an existing Host call, not
                        // authority from caller-supplied execution metadata.
                        if route.request.name != request.call.request.name
                            || route.request.arguments != request.call.request.arguments
                            || route.invocation.is_some_and(|id| id != request.call.id)
                        {
                            return None;
                        }
                        if route.invocation.is_none() {
                            debug!(invocation = ?request.call.id, tool_call_id = %route.request.id, tool = %route.request.name, "Associated MCP invocation with Host tool call");
                        }
                        route.invocation = Some(request.call.id);
                        Some(route.sender.clone())
                    })
                };
                if let Some(sender) = sender {
                    drop(sender.send(request).await);
                }
                // An unassociated call loses its reply sender and fails closed.
            }
        });
        let sinks = Sinks::default();
        let progress_sinks = sinks.clone();
        let progress = tokio::spawn(async move {
            loop {
                match stderr.recv().await {
                    Ok(line) => {
                        let sink = locked(&progress_sinks).get(&line.id).cloned();
                        if let Some(sink) = sink {
                            sink(&line.line);
                        }
                    }
                    Err(ProgressError::Lagged(_)) => {}
                    Err(ProgressError::Closed) => break,
                }
            }
        });
        let source = Self {
            peer: client.peer().clone(),
            service: endpoint.service(),
            definitions: definitions
                .iter()
                .map(|d| (d.name.clone(), d.clone()))
                .collect(),
            calls: Calls::default(),
            routes,
            sinks,
        };
        Ok((source, ExecutionOwner {
            endpoint: Some(endpoint),
            client: Some(client),
            router,
            progress,
        }))
    }
}

impl ExecutorSource for TerminalExecutorSource {
    fn create(
        &self,
        request: ToolCallRequest,
        config: ToolConfigWithDefaults,
    ) -> Option<Box<dyn Executor>> {
        self.definitions.get(&request.name)?;
        let (sender, receiver) = mpsc::channel(8);
        let key = format!("{:032x}", random::<u128>());
        locked(&self.routes).insert(key.clone(), Route {
            request: request.clone(),
            invocation: None,
            sender,
        });
        let state = Arc::new(Mutex::new(PendingCall {
            receiver,
            task: None,
            input: None,
            prepare: None,
            release: None,
            review: None,
            record: None,
            id: None,
            finished: false,
        }));
        locked(&self.calls).insert(request.id.clone(), state.clone());
        Some(Box::new(ToolExecutor {
            request,
            config,
            key,
            peer: self.peer.clone(),
            service: self.service.clone(),
            state,
            formatted: None,
            sinks: self.sinks.clone(),
        }))
    }

    fn acknowledge(&self, response: ToolCallResponse) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            let call = locked(&self.calls).remove(&response.id);
            let Some(call) = call else {
                return Ok(());
            };
            let mut call = call.lock().await;
            let result = call.acknowledge(response.result).await;
            if let Some(id) = call.id {
                locked(&self.sinks).remove(&id);
            }
            locked(&self.routes).retain(|_, route| route.request.id != response.id);
            result
        })
    }
}

struct PendingCall {
    receiver: mpsc::Receiver<HostRequest>,
    task: Option<JoinHandle<Result<CallToolResult, McpCallError>>>,
    input: Option<(String, Reply<InputAnswer>)>,
    prepare: Option<Reply<Admission>>,
    release: Option<Reply<ReleaseDecision>>,
    review: Option<Reply<TextResult>>,
    record: Option<Reply<()>>,
    id: Option<InvocationId>,
    finished: bool,
}

#[expect(
    clippy::large_enum_variant,
    reason = "Each Host interaction is consumed immediately without an additional allocation"
)]
enum Received {
    Interaction(Interaction),
    Finished(TextResult),
}

impl PendingCall {
    async fn next(&mut self) -> Result<Received, String> {
        let task = self.task.as_mut().ok_or("MCP call has not started")?;
        tokio::select! {
            request = self.receiver.recv() => {
                let request = request.ok_or("MCP Host interaction channel closed")?;
                self.id = Some(request.call.id);
                Ok(Received::Interaction(request.interaction))
            }
            result = task => {
                self.task = None;
                self.finished = true;
                let result = result.map_err(|error| error.to_string())?;
                Ok(Received::Finished(result.map_or_else(|error| Err(error.to_string()), |result| text_result(&result))))
            }
        }
    }

    async fn acknowledge(&mut self, result: TextResult) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        if let Some(reply) = self.prepare.take() {
            drop(reply.send(Ok(Admission::Complete {
                result: result.clone(),
            })));
        } else if let Some(reply) = self.release.take() {
            drop(reply.send(Ok(ReleaseDecision::Complete {
                result: result.clone(),
            })));
        } else if let Some((_, reply)) = self.input.take() {
            drop(reply.send(Ok(InputAnswer::Complete {
                result: result.clone(),
            })));
        } else if let Some(reply) = self.review.take() {
            drop(reply.send(Ok(result.clone())));
        }
        if let Some(reply) = self.record.take() {
            drop(reply.send(Ok(())));
        }
        loop {
            match self.next().await? {
                Received::Interaction(Interaction::Review { reply, .. }) => {
                    drop(reply.send(Ok(result.clone())));
                }
                Received::Interaction(Interaction::Record {
                    reply,
                    result: delivered,
                    ..
                }) => {
                    if delivered != result {
                        return Err("MCP result differs from the recorded response".into());
                    }
                    drop(reply.send(Ok(())));
                }
                Received::Finished(delivered) => {
                    if delivered != result {
                        return Err("MCP response differs from the recorded response".into());
                    }
                    return Ok(());
                }
                Received::Interaction(_) => {
                    return Err("Unexpected MCP interaction during recording".into());
                }
            }
        }
    }
}

/// Represents one logical MCP call, including its pending Host interactions.
pub struct ToolExecutor {
    request: ToolCallRequest,
    config: ToolConfigWithDefaults,
    key: String,
    peer: Peer<RoleClient>,
    service: Arc<Service>,
    state: Arc<Mutex<PendingCall>>,
    formatted: Option<Result<String, String>>,
    sinks: Sinks,
}

#[async_trait]
impl Executor for ToolExecutor {
    fn tool_id(&self) -> &str {
        &self.request.id
    }
    fn tool_name(&self) -> &str {
        &self.request.name
    }
    fn arguments(&self) -> &Map<String, Value> {
        &self.request.arguments
    }
    fn formats_arguments(&self) -> bool {
        true
    }
    fn formatted_arguments(&self) -> Option<&TextResult> {
        self.formatted.as_ref()
    }
    fn permission_info(&self) -> Option<PermissionInfo> {
        let run_mode = self.config.run();
        if matches!(run_mode, RunMode::Unattended | RunMode::Skip) {
            return None;
        }
        Some(PermissionInfo {
            tool_id: self.request.id.clone(),
            tool_name: self.request.name.clone(),
            tool_source: self.config.source().clone(),
            run_mode,
            arguments: self.request.arguments.clone().into(),
        })
    }
    fn set_arguments(&mut self, args: Value) {
        if let Value::Object(arguments) = args {
            self.request.arguments = arguments;
        }
    }

    async fn prepare(
        &mut self,
        render_arguments: bool,
    ) -> Result<Option<ToolCallResponse>, String> {
        let mut state = self.state.lock().await;
        if state.task.is_some() || state.finished {
            return Err("MCP call was prepared twice".into());
        }
        let mut params = CallToolRequestParams::new(self.request.name.clone());
        params.arguments = Some(self.request.arguments.clone());
        params.meta = Some(Meta(Map::from_iter([(
            CORRELATION_KEY.into(),
            self.key.clone().into(),
        )])));
        let peer = self.peer.clone();
        state.task = Some(tokio::spawn(async move { peer.call_tool(params).await }));
        loop {
            match state.next().await? {
                Received::Interaction(Interaction::RenderArguments { reply }) => {
                    drop(reply.send(Ok(render_arguments)));
                }
                Received::Interaction(Interaction::Prepare {
                    arguments,
                    formatted_arguments,
                    reply,
                    ..
                }) => {
                    self.request.arguments = arguments;
                    self.formatted = formatted_arguments;
                    state.prepare = Some(reply);
                    return Ok(None);
                }
                Received::Interaction(Interaction::Record { result, reply, .. }) => {
                    state.record = Some(reply);
                    return Ok(Some(ToolCallResponse {
                        id: self.request.id.clone(),
                        result,
                    }));
                }
                Received::Finished(result) => {
                    return Ok(Some(ToolCallResponse {
                        id: self.request.id.clone(),
                        result,
                    }));
                }
                Received::Interaction(_) => {
                    return Err("Unexpected MCP preparation interaction".into());
                }
            }
        }
    }

    async fn approve(&mut self) -> Result<(), String> {
        let mut state = self.state.lock().await;
        let reply = state
            .prepare
            .take()
            .ok_or("MCP call is not awaiting approval")?;
        reply
            .send(Ok(Admission::Run {
                arguments: self.request.arguments.clone(),
            }))
            .map_err(|_| "MCP approval expired")?;
        match state.next().await? {
            Received::Interaction(Interaction::Release {
                arguments,
                formatted_arguments,
                reply,
            }) => {
                self.request.arguments = arguments;
                self.formatted = formatted_arguments;
                state.release = Some(reply);
                Ok(())
            }
            Received::Finished(Err(error)) => Err(error),
            _ => Err("MCP call did not reach the release barrier".into()),
        }
    }

    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        _: &Client,
        _: &Utf8Path,
        cancellation: CancellationToken,
        stderr: Option<StderrSink>,
    ) -> ExecutorResult {
        let mut state = self.state.lock().await;
        let result = async {
            if let (Some(id), Some(stderr)) = (state.id, stderr) {
                locked(&self.sinks).insert(id, stderr);
            }
            if let Some(reply) = state.release.take() {
                reply
                    .send(Ok(ReleaseDecision::Execute))
                    .map_err(|_| "MCP release expired")?;
            }
            if let Some((id, reply)) = state.input.take() {
                let answer = answers
                    .get(&id)
                    .ok_or("Missing answer to pending MCP inquiry")?
                    .clone();
                reply
                    .send(Ok(InputAnswer::Answer(answer)))
                    .map_err(|_| "MCP inquiry expired")?;
            }
            match state.next().await? {
                Received::Interaction(Interaction::Input {
                    request,
                    supporting,
                    answers,
                    reply,
                }) => {
                    let question = question(request, &supporting)?;
                    state.input = Some((question.id.to_string(), reply));
                    Ok(ExecutorResult::NeedsInput {
                        tool_id: self.request.id.clone(),
                        tool_name: self.request.name.clone(),
                        source: InquirySource::tool(&self.request.name),
                        question,
                        accumulated_answers: answers,
                    })
                }
                Received::Interaction(Interaction::Review { result, reply, .. }) => {
                    state.review = Some(reply);
                    Ok(ExecutorResult::Completed(ToolCallResponse {
                        id: self.request.id.clone(),
                        result,
                    }))
                }
                Received::Interaction(Interaction::Record { result, reply, .. }) => {
                    state.record = Some(reply);
                    Ok(ExecutorResult::Completed(ToolCallResponse {
                        id: self.request.id.clone(),
                        result,
                    }))
                }
                Received::Finished(result) => Ok(ExecutorResult::Completed(ToolCallResponse {
                    id: self.request.id.clone(),
                    result,
                })),
                Received::Interaction(_) => Err("Unexpected MCP execution interaction".into()),
            }
        };
        let result: Result<ExecutorResult, String> = tokio::select! {
            biased;
            () = cancellation.cancelled() => Err("Tool execution cancelled.".into()),
            result = result => result,
        };
        if result.is_err() {
            if let Some(id) = state.id {
                self.service.cancel_call(id);
            }
            state.finished = true;
        }
        result.unwrap_or_else(|error| {
            ExecutorResult::Completed(ToolCallResponse {
                id: self.request.id.clone(),
                result: Err(error),
            })
        })
    }
}

fn question(request: InputRequest, supporting: &[ContentBlock]) -> Result<Question, String> {
    let answer_type = if request.secret {
        AnswerType::Secret
    } else if request.schema.get("type").and_then(Value::as_str) == Some("boolean") {
        AnswerType::Boolean
    } else if let Some(options) = request.schema.get("enum").and_then(Value::as_array) {
        AnswerType::Select {
            options: options
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or("Non-string inquiry choice".to_owned())
                })
                .collect::<Result<_, _>>()?,
        }
    } else if request.schema.get("type").and_then(Value::as_str) == Some("string") {
        AnswerType::Text
    } else {
        return Err("Unsupported tool inquiry schema".into());
    };
    let preamble = supporting
        .iter()
        .filter_map(ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut question = Question::text(request.id.to_string(), request.label)
        .map_err(|error| error.to_string())?
        .with_answer_type(answer_type);
    question.pre_amble = (!preamble.is_empty()).then_some(preamble);
    question.default = request.default;
    Ok(question)
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;
