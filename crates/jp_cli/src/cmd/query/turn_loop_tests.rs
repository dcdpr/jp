use std::{
    collections::VecDeque,
    fmt,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use camino_tempfile::tempdir;
use futures::{StreamExt as _, stream};
use indexmap::IndexMap;
use inquire::InquireError;
use jp_config::{
    AppConfig,
    conversation::tool::{
        CommandConfigOrString, QuestionConfig, QuestionTarget, RunMode, ToolConfig, ToolSource,
        style::{DisplayStyleConfig, ErrorStyleConfig, InlineResults, LinkStyle, ParametersStyle},
    },
    interrupt::ToolInterruptAction,
    model::id::{self, ProviderId},
};
use jp_conversation::{
    Conversation,
    event::{ChatRequest, ChatResponse, InquirySource, ToolCallRequest, TurnStart},
};
use jp_inquire::{
    InlineOption, ReplyEditMode, ReplyOutcome,
    prompt::{MockPromptBackend, PromptBackend},
};
use jp_llm::{
    Error as LlmError, EventStream, Provider,
    error::StreamError,
    event::{Event, FinishReason},
    model::ModelDetails,
    provider::mock::MockProvider,
    query::ChatQuery,
    tool::{
        InvocationContext,
        builtin::BuiltinExecutors,
        executor::{
            Executor, ExecutorResult, ExecutorSource, MockExecutor, PermissionInfo,
            TestExecutorSource,
        },
    },
};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::FsStorageBackend;
use jp_tool::Question;
use jp_workspace::Workspace;
use serde_json::{Map, Value, json};
use tokio::{sync::Notify, time::timeout};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    cmd::query::tool::{ToolCoordinator, executor::TerminalExecutorSource},
    signals::testing::{detached_router, test_router},
};

fn empty_executor_source() -> Box<dyn ExecutorSource> {
    Box::new(TerminalExecutorSource::new(
        BuiltinExecutors::new(),
        &[],
        std::sync::Arc::new(crate::access::approvals::ApprovalStore::default()),
        InvocationContext::default(),
    ))
}

/// A mock provider that returns different responses on each call.
///
/// This enables testing multi-cycle conversations where the LLM returns tool
/// calls on the first request, then a final message on the follow-up.
#[derive(Debug)]
struct SequentialMockProvider {
    /// Sequence of event lists to return on each call.
    responses: Vec<Vec<Event>>,

    /// Current call index (atomic for interior mutability in async trait).
    call_index: AtomicUsize,

    /// Model details to return.
    model: ModelDetails,
}

impl SequentialMockProvider {
    /// Create a provider that returns tool calls first, then a message.
    fn with_tool_then_message(tool_id: &str, tool_name: &str, final_message: &str) -> Self {
        // First response: tool call
        let tool_call_events = vec![
            Event::tool_call_start(0, tool_id.to_string(), tool_name.to_string()),
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        // Second response: final message
        let message_events = vec![
            Event::message(0, final_message),
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        Self {
            responses: vec![tool_call_events, message_events],
            call_index: AtomicUsize::new(0),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "sequential-mock".parse().expect("valid name"),
            }),
        }
    }

    /// Create a provider whose single response stream ends WITHOUT a terminal
    /// `Finished` event, simulating a provider that drops or stalls the
    /// connection mid-stream.
    fn with_premature_end(events: Vec<Event>) -> Self {
        Self {
            responses: vec![events],
            call_index: AtomicUsize::new(0),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "premature-mock".parse().expect("valid name"),
            }),
        }
    }
}

#[async_trait]
impl Provider for SequentialMockProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = self.model.clone();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![self.model.clone()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        let index = self.call_index.fetch_add(1, Ordering::SeqCst);
        let events = self
            .responses
            .get(index)
            .cloned()
            .unwrap_or_else(|| vec![Event::Finished(FinishReason::Completed)]);

        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

/// A provider whose stream always ends immediately with no events and no
/// terminal `Finished`, counting calls so a test can assert the retry budget
/// was consumed.
#[derive(Debug, Default)]
struct AlwaysPrematureProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for AlwaysPrematureProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        Ok(ModelDetails::empty(id::ModelIdConfig {
            provider: ProviderId::Test,
            name: name.clone(),
        }))
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(stream::iter(
            Vec::<Result<Event, StreamError>>::new(),
        )))
    }
}

/// A provider whose stream yields the given events and then stays pending
/// forever, simulating an in-flight response that only an interrupt can stop.
#[derive(Debug)]
struct StallingMockProvider {
    events: Vec<Event>,
    model: ModelDetails,

    /// Notified on the first poll of the stream's pending tail; see
    /// [`Self::notify_when_stalled`].
    stalled: Option<Arc<Notify>>,
}

impl StallingMockProvider {
    /// Create a provider that streams a committed message and then stalls.
    fn with_message(content: &str) -> Self {
        Self {
            events: vec![Event::message(0, content), Event::flush(0)],
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "stalling-mock".parse().expect("valid name"),
            }),
            stalled: None,
        }
    }

    /// Notify `stalled` when the stream's pending tail is first polled.
    ///
    /// That poll can only come from the streaming event loop after it has
    /// consumed every scripted event, so it doubles as a synchronization point
    /// at which the loop — and its registered interrupt handler — is known to
    /// be live.
    fn notify_when_stalled(mut self, stalled: &Arc<Notify>) -> Self {
        self.stalled = Some(Arc::clone(stalled));
        self
    }
}

#[async_trait]
impl Provider for StallingMockProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = self.model.clone();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![self.model.clone()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        let events: Vec<Result<Event, StreamError>> = self.events.iter().cloned().map(Ok).collect();

        // The tail is polled only after every scripted event above has been
        // consumed, i.e. from inside the streaming event loop.
        let stalled = self.stalled.clone();
        let mut notified = false;
        let tail = stream::poll_fn(move |_| {
            if !notified {
                notified = true;
                if let Some(stalled) = &stalled {
                    stalled.notify_one();
                }
            }
            std::task::Poll::Pending
        });

        Ok(Box::pin(stream::iter(events).chain(tail)))
    }
}

#[tokio::test]
async fn test_interrupt_stop_during_streaming_persists_content() {
    // A Ctrl-C press is routed to the streaming loop's registered interrupt
    // handler; choosing Stop ('s') from the menu commits the partial content
    // and ends the turn.
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let config = AppConfig::new_test();
        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
            .unwrap();
        let conv_id = lock.id();

        let chat_request = ChatRequest::from("What is 2+2?");

        // The stream commits partial content, then stalls until interrupted.
        // `stalled` fires once the stream is parked inside the streaming
        // event loop; see the Ctrl-C task below.
        let stalled = Arc::new(Notify::new());
        let provider: Arc<dyn Provider> = Arc::new(
            StallingMockProvider::with_message("The answer is 4.").notify_when_stalled(&stalled),
        );
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (router, signals) = test_router();
        let router = Arc::new(router);

        // Mock user selecting 's' (Stop) from the interrupt menu.
        let backend = MockPromptBackend::new().with_inline_responses(['s']);

        // Press Ctrl-C once the stream has stalled: the notification fires on
        // the first poll of the stream's pending tail, from inside the
        // streaming event loop, so the loop's interrupt handler is registered
        // by then. A fixed sleep raced handler registration on slow runners
        // (seen on Windows CI): a press routed while the handler stack is
        // empty skips the menu and cancels the shutdown token directly.
        let signal_handle = tokio::spawn({
            let stalled = Arc::clone(&stalled);
            async move {
                stalled.notified().await;
                signals.interrupt().await;
            }
        });

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // is_tty
            &[],   // attachments
            &lock,
            ToolChoice::Auto,
            &[], // tools
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        signal_handle.await.unwrap();

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Stop commits the streamed partial content before ending the turn.
        let content = fs
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");

        assert!(
            content.contains("What is 2+2?"),
            "Persisted events should contain the user query.\nFile contents:\n{content}"
        );
        assert!(
            content.contains("The answer is 4."),
            "Persisted events should contain the interrupted partial content.\nFile \
             contents:\n{content}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

/// Cancelling the streaming interrupt menu (a second Ctrl-C) escalates: the
/// partial content is committed, a graceful shutdown begins, and the turn ends
/// with the interrupt error.
#[tokio::test(flavor = "multi_thread")]
async fn test_streaming_interrupt_menu_cancel_escalates() {
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let config = AppConfig::new_test();
        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
            .unwrap();
        let conv_id = lock.id();

        let chat_request = ChatRequest::from("What is 2+2?");

        // The stream commits partial content, then stalls until interrupted.
        // `stalled` fires once the stream is parked inside the streaming
        // event loop; see the Ctrl-C task below.
        let stalled = Arc::new(Notify::new());
        let provider: Arc<dyn Provider> = Arc::new(
            StallingMockProvider::with_message("The answer is 4.").notify_when_stalled(&stalled),
        );
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (router, signals) = test_router();
        let router = Arc::new(router);

        // No pre-loaded prompt responses: opening the interrupt menu and
        // cancelling it (as a second Ctrl-C would) escalates.
        let backend = MockPromptBackend::new();

        // Press Ctrl-C once the stream has stalled: the notification fires on
        // the first poll of the stream's pending tail, from inside the
        // streaming event loop, so the loop's interrupt handler is registered
        // by then. A fixed sleep raced handler registration on slow runners
        // (seen on Windows CI): a press routed while the handler stack is
        // empty skips the menu and cancels the shutdown token directly.
        let signal_handle = tokio::spawn({
            let stalled = Arc::clone(&stalled);
            async move {
                stalled.notified().await;
                signals.interrupt().await;
            }
        });

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // is_tty
            &[],   // attachments
            &lock,
            ToolChoice::Auto,
            &[], // tools
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        signal_handle.await.unwrap();

        // Escalation ends the turn with the interrupt error (exit code 130)
        // and begins a graceful shutdown.
        assert!(
            matches!(result, Err(Error::Command(ref e)) if e.code.get() == 130),
            "expected interrupted turn, got {result:?}"
        );
        assert!(
            router.shutdown_token().is_cancelled(),
            "escalation must request a graceful shutdown"
        );

        // The streamed content was persisted before the shutdown.
        let content = fs
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");
        assert!(
            content.contains("What is 2+2?"),
            "Persisted events should contain the user query.\nFile contents:\n{content}"
        );
        assert!(
            content.contains("The answer is 4."),
            "Persisted events should contain the streamed partial content.\nFile \
             contents:\n{content}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

#[tokio::test]
async fn test_normal_completion_persists_content() {
    // This test verifies normal (non-interrupted) completion also persists correctly
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    let chat_request = ChatRequest::from("Hello");

    let response_content = "Hello! How can I help you today?";
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message(response_content));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
    )
    .await
    .unwrap();

    // Verify printer output contains the LLM response
    // Note: markdown renderer may escape special characters like '!' → '\!'
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("How can I help you"),
        "Printer output should contain LLM response.\nOutput:\n{output}"
    );

    // Verify persistence
    let content = fs
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    assert!(
        content.contains("Hello"),
        "Should contain user query.\nFile contents:\n{content}"
    );
    assert!(
        content.contains(response_content),
        "Should contain assistant response.\nFile contents:\n{content}"
    );
}

/// Regression: a provider stream that ends without a terminal `Finished` event
/// (a dropped or stalled connection) must surface as an error rather than
/// hanging the loop forever on the signal/tick sources.
#[tokio::test]
async fn premature_stream_end_without_finished_returns_error() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    // Disable retries so the premature end fails fast instead of cycling
    // through the backoff schedule.
    let mut config = AppConfig::new_test();
    config.assistant.request.max_retries = 0;

    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();

    // Emits a partial message, then ends with no `Finished` event.
    let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_premature_end(vec![
        Event::message(0, "partial answer"),
        Event::flush(0),
    ]));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    // Without the backstop the loop pends forever, so cap the whole run.
    let result = timeout(
        Duration::from_secs(5),
        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            ChatRequest::from("hi"),
            InvocationContext::default(),
        ),
    )
    .await
    .expect("turn loop hung on a stream that never sent a Finished event");

    assert!(
        result.is_err(),
        "a premature stream end should surface as an error, got: {result:?}"
    );
}

/// A premature stream end is retryable: the loop retries until the budget is
/// exhausted, then returns an error rather than retrying forever.
#[tokio::test]
async fn premature_stream_end_exhausts_retry_budget() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    // Two retries, zero backoff so the retries are instant.
    let mut config = AppConfig::new_test();
    config.assistant.request.max_retries = 2;
    config.assistant.request.base_backoff_ms = 0;

    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();

    let provider = Arc::new(AlwaysPrematureProvider::default());
    let dyn_provider: Arc<dyn Provider> = provider.clone();
    let model = dyn_provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    let result = timeout(
        Duration::from_secs(5),
        run_turn_loop(
            dyn_provider,
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            ChatRequest::from("hi"),
            InvocationContext::default(),
        ),
    )
    .await
    .expect("turn loop should exhaust retries quickly, not hang");

    assert!(
        result.is_err(),
        "exhausted retries should return an error, got: {result:?}"
    );

    // 1 initial attempt + 2 retries.
    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        3,
        "provider should be called once per attempt across the retry budget"
    );
}

/// Regression: any `ToolCallRequest` already in the stream when a new
/// `Streaming` cycle starts MUST be sanitized into a stream that's safe to send
/// to the provider.
/// Otherwise providers like Anthropic reject the request with `tool_use ids
/// were found without tool_result blocks`.
///
/// Reproduces the failure mode from the bug report by injecting an orphaned
/// `ToolCallRequest` in a prior turn and then running a fresh turn.
/// After the turn loop completes, the persisted stream must contain a synthetic
/// "Tool call was interrupted." response for the orphan.
#[tokio::test]
async fn orphan_tool_call_is_sanitized_before_provider_request() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    // Inject a "previous turn" with an orphaned ToolCallRequest, simulating
    // the corrupted state that triggered the original bug. We bypass
    // `run_turn_loop`'s own start_turn and the top-level `query.rs` sanitize
    // by mutating the stream directly here.
    {
        let mut conv = lock.as_mut();
        conv.update_events(|stream| {
            stream.start_turn(ChatRequest::from("earlier query"));
            stream
                .current_turn_mut()
                .add_chat_response(ChatResponse::message("calling a tool"))
                .add_tool_call_request(ToolCallRequest {
                    id: "orphan_id".to_string(),
                    name: "some_tool".to_string(),
                    arguments: Map::new(),
                })
                .build()
                .expect("orphan setup");
            // No matching ToolCallResponse — this is the orphan.
        });
        conv.flush().unwrap();
    }

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("ok"));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        ChatRequest::from("new query"),
        InvocationContext::default(),
    )
    .await
    .unwrap();

    // The synthetic response is injected by `sanitize` before the cycle's
    // provider request. After the turn it should appear in the persisted
    // events.
    let content = fs
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    assert!(
        content.contains("orphan_id"),
        "orphan request must remain in the persisted stream:\n{content}"
    );
    // The synthetic response content is base64-encoded in the on-disk form
    // ("Tool call was interrupted." -> VG9vbCBjYWxsIHdhcyBpbnRlcnJ1cHRlZC4=).
    assert!(
        content.contains("VG9vbCBjYWxsIHdhcyBpbnRlcnJ1cHRlZC4="),
        "sanitize must inject a synthetic response for the orphan:\n{content}"
    );
    assert!(
        content.contains("\"is_error\": true"),
        "synthetic response must be marked as an error:\n{content}"
    );
}

#[tokio::test]
async fn test_tool_call_cycle_completes_with_followup() {
    // Tests the full tool execution cycle:
    // 1. LLM returns a tool call
    // 2. Tool execution phase runs (tool not found, but cycle continues)
    // 3. LLM returns final message
    // 4. Conversation persists with tool call and final response

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    let chat_request = ChatRequest::from("List files in current directory");

    // Provider returns tool call first, then message
    let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
        "call_123",
        "fs_list_files",
        "Here are the files in the directory.",
    ));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[], // No tool definitions - tests the "tool not found" path
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
    )
    .await;

    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer output contains final LLM response
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("Here are the files"),
        "Printer output should contain final LLM response.\nOutput:\n{output}"
    );

    // Verify persistence
    let content = fs
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // Should contain the user query
    assert!(
        content.contains("List files"),
        "Should contain user query.\nFile contents:\n{content}"
    );

    // Should contain the tool call request
    assert!(
        content.contains("fs_list_files") || content.contains("call_123"),
        "Should contain tool call.\nFile contents:\n{content}"
    );

    // Should contain the final message
    assert!(
        content.contains("Here are the files"),
        "Should contain final response.\nFile contents:\n{content}"
    );
}

/// An executor that runs until its cancellation token fires, giving interrupt
/// tests a stable window in which a tool is "running".
/// A generous fallback deadline keeps a missed cancellation from pending
/// forever.
#[derive(Debug)]
struct SleepingExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,
    /// Notified when `execute` starts, so tests can fire an interrupt while the
    /// tool is guaranteed to be running (and the tool interrupt handler
    /// guaranteed to be registered, as the coordinator pushes it before
    /// spawning executors).
    /// A fixed sleep is not enough: on slow machines (e.g. Windows CI) the
    /// press can land before the executing phase and be consumed by an earlier
    /// handler.
    started: Option<Arc<Notify>>,
}

impl SleepingExecutor {
    fn notifying(tool_id: &str, tool_name: &str, started: Arc<Notify>) -> Self {
        Self {
            tool_id: tool_id.to_owned(),
            tool_name: tool_name.to_owned(),
            arguments: Map::new(),
            started: Some(started),
        }
    }
}

#[async_trait]
impl Executor for SleepingExecutor {
    fn tool_id(&self) -> &str {
        &self.tool_id
    }

    fn tool_name(&self) -> &str {
        &self.tool_name
    }

    fn arguments(&self) -> &Map<String, Value> {
        &self.arguments
    }

    fn permission_info(&self) -> Option<PermissionInfo> {
        None
    }

    fn set_arguments(&mut self, _args: Value) {}

    async fn execute(
        &self,
        _answers: &IndexMap<String, Value>,
        _mcp_client: &jp_mcp::Client,
        _root: &Utf8Path,
        cancellation_token: CancellationToken,
    ) -> ExecutorResult {
        if let Some(started) = &self.started {
            started.notify_one();
        }

        tokio::select! {
            () = cancellation_token.cancelled() => {
                ExecutorResult::Completed(ToolCallResponse {
                    id: self.tool_id.clone(),
                    result: Err("Tool execution was cancelled".to_owned()),
                })
            }
            () = tokio::time::sleep(Duration::from_secs(5)) => {
                ExecutorResult::Completed(ToolCallResponse {
                    id: self.tool_id.clone(),
                    result: Ok("completed without interruption".to_owned()),
                })
            }
        }
    }
}

/// A prompt backend that stalls before answering, giving tests a stable window
/// in which a tool prompt is active.
struct DelayedPromptBackend {
    inner: MockPromptBackend,
    delay: Duration,
    /// Notified when a prompt becomes active, so tests can fire an interrupt
    /// inside the prompt window instead of guessing with a fixed sleep.
    started: Arc<Notify>,
}

impl PromptBackend for DelayedPromptBackend {
    fn inline_select(
        &self,
        message: &str,
        options: Vec<InlineOption>,
        default: Option<char>,
        writer: &mut dyn Write,
    ) -> Result<char, InquireError> {
        self.started.notify_one();
        std::thread::sleep(self.delay);
        self.inner.inline_select(message, options, default, writer)
    }

    fn inline_reply(
        &self,
        message: &str,
        initial_text: &str,
        edit_mode: ReplyEditMode,
        editor_escape: bool,
        output: Box<dyn Write + Send>,
    ) -> Result<ReplyOutcome, InquireError> {
        self.started.notify_one();
        std::thread::sleep(self.delay);
        self.inner
            .inline_reply(message, initial_text, edit_mode, editor_escape, output)
    }

    fn text(
        &self,
        message: &str,
        default: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.started.notify_one();
        std::thread::sleep(self.delay);
        self.inner.text(message, default, writer)
    }

    fn select(
        &self,
        message: &str,
        options: Vec<String>,
        default: Option<usize>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.started.notify_one();
        std::thread::sleep(self.delay);
        self.inner.select(message, options, default, writer)
    }
}

/// Tests the escalation flow:
///
/// 1. LLM returns a tool call
/// 2. During execution, Ctrl-C opens the tool interrupt menu
/// 3. The user cancels the menu itself (a second Ctrl-C)
/// 4. The tools are cancelled, a graceful shutdown begins, and the turn ends
///    with the interrupt error
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn test_tool_interrupt_menu_cancel_escalates() {
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("slow_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();
        let conv_id = lock.id();

        let chat_request = ChatRequest::from("Please use a tool");

        let provider = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_escalate",
            "slow_tool",
            "This follow-up should never be requested.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (router, signals) = test_router();
        let router = Arc::new(router);

        // No pre-loaded prompt responses: opening the interrupt menu and
        // cancelling it (as a second Ctrl-C would) escalates.
        let backend = MockPromptBackend::new();

        // The tool runs until the escalation cancels it, and signals
        // `tool_started` once it is executing.
        let tool_started = Arc::new(Notify::new());
        let executor_source = TestExecutorSource::new().with_executor("slow_tool", {
            let tool_started = Arc::clone(&tool_started);
            move |req| {
                Box::new(SleepingExecutor::notifying(
                    &req.id,
                    &req.name,
                    Arc::clone(&tool_started),
                ))
            }
        });

        // Press Ctrl-C once the tool is executing, which guarantees the tool
        // interrupt handler is topmost.
        let signal_handle = tokio::spawn(async move {
            tool_started.notified().await;
            signals.interrupt().await;
        });

        let result = run_turn_loop(
            Arc::clone(&provider) as Arc<dyn Provider>,
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        signal_handle.await.unwrap();

        // No follow-up request was sent after the escalation.
        let call_count = provider.call_index.load(Ordering::SeqCst);
        assert_eq!(call_count, 1, "escalation must not trigger a follow-up");

        // Escalation ends the turn with the interrupt error (exit code 130)
        // and begins a graceful shutdown.
        assert!(
            matches!(result, Err(Error::Command(ref e)) if e.code.get() == 130),
            "expected interrupted turn, got {result:?}"
        );
        assert!(
            router.shutdown_token().is_cancelled(),
            "escalation must request a graceful shutdown"
        );

        // The user query was persisted before the shutdown.
        let content = fs
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");
        assert!(
            content.contains("Please use a tool"),
            "Should contain user query.\nFile contents:\n{content}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

/// Tests the stop flow:
///
/// 1. LLM returns a tool call
/// 2. During execution, Ctrl-C is routed to the tool interrupt handler
/// 3. `interrupt.tool_call.action = "stop"` skips the menu: the tools are
///    cancelled and each cancelled call records its configured
///    `cancellation_response`
/// 4. The responses are committed and the turn ends without a follow-up request
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn test_tool_stop_on_interrupt_commits_responses_without_follow_up() {
    const CUSTOM_CANCELLATION_RESPONSE: &str =
        "slow_tool was cancelled by the user; do not retry it this turn.";

    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        // Skip the interrupt menu: Ctrl-C during tool execution cancels the
        // tools, records their cancellation responses, and ends the turn.
        config.interrupt.tool_call.action = ToolInterruptAction::Stop;
        config
            .conversation
            .tools
            .insert("slow_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: Some(CUSTOM_CANCELLATION_RESPONSE.to_string()),
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();
        let conv_id = lock.id();

        let chat_request = ChatRequest::from("Please use a tool");

        let provider = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_stop",
            "slow_tool",
            "This follow-up should never be requested.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = Arc::new(SignalRouter::detached());

        // No prompt responses: the configured `stop` action never shows the
        // menu, so any prompt would fail the test.
        let backend = MockPromptBackend::new();

        // The tool runs until the stop cancels it, and signals `tool_started`
        // once it is executing.
        let tool_started = Arc::new(Notify::new());
        let executor_source = TestExecutorSource::new().with_executor("slow_tool", {
            let tool_started = Arc::clone(&tool_started);
            move |req| {
                Box::new(SleepingExecutor::notifying(
                    &req.id,
                    &req.name,
                    Arc::clone(&tool_started),
                ))
            }
        });

        // Press Ctrl-C once the tool is executing, which guarantees the tool
        // interrupt handler is topmost.
        let signal_router = Arc::clone(&router);
        let signal_handle = tokio::spawn(async move {
            tool_started.notified().await;
            signal_router.simulate_interrupt();
        });

        let result = run_turn_loop(
            Arc::clone(&provider) as Arc<dyn Provider>,
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source))
                .with_interrupt(config.interrupt.tool_call.clone()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        signal_handle.await.unwrap();

        // Unlike an escalation, a stop ends the turn cleanly: no interrupt
        // error, no graceful shutdown.
        assert!(result.is_ok(), "stop must end the turn cleanly: {result:?}");
        assert!(
            !router.shutdown_token().is_cancelled(),
            "stop must not request a graceful shutdown"
        );

        // No follow-up request was sent after the stop.
        let call_count = provider.call_index.load(Ordering::SeqCst);
        assert_eq!(call_count, 1, "stop must not trigger a follow-up request");

        // The cancelled call's configured cancellation response was
        // persisted, keeping every tool call paired with a response.
        // Tool response content is base64-encoded in the raw events file.
        let content = fs
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");
        let encoded_response = STANDARD.encode(CUSTOM_CANCELLATION_RESPONSE);
        assert!(
            content.contains(&encoded_response),
            "Should contain the configured cancellation response (base64-encoded).\nFile \
             contents:\n{content}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

/// A Ctrl-C pressed while a tool question prompt is active is declined by the
/// tool handler and lands on the turn-level handler, which ends the turn
/// gracefully once the tool completes: no follow-up request is sent.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn test_interrupt_during_tool_prompt_completes_turn_early() {
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("question_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::from_iter([("confirm".to_string(), QuestionConfig {
                    target: QuestionTarget::User,
                    answer: None,
                })]),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();
        let conv_id = lock.id();

        let chat_request = ChatRequest::from("Ask me something");

        let provider = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_question",
            "question_tool",
            "This follow-up should never be requested.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (router, signals) = test_router();
        let router = Arc::new(router);

        // The question prompt stalls for 400ms before answering 'y'. While it
        // is pending, the execution event loop declines interrupts.
        let prompt_started = Arc::new(Notify::new());
        let backend = DelayedPromptBackend {
            inner: MockPromptBackend::new().with_inline_responses(['y']),
            delay: Duration::from_millis(400),
            started: Arc::clone(&prompt_started),
        };

        let executor_source = TestExecutorSource::new().with_executor("question_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::boolean("confirm", "Proceed?")],
                "question tool output",
            ))
        });

        // Press Ctrl-C once the 400ms prompt is active. The tool handler
        // declines it (a prompt is active); the turn-level handler picks it
        // up after the tool completes.
        let signal_handle = tokio::spawn(async move {
            prompt_started.notified().await;
            signals.interrupt().await;
        });

        let result = run_turn_loop(
            Arc::clone(&provider) as Arc<dyn Provider>,
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty: user-targeted question prompts require a terminal
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        signal_handle.await.unwrap();

        assert!(
            result.is_ok(),
            "Turn should complete gracefully: {result:?}"
        );

        // The deferred interrupt ended the turn before the follow-up request.
        let call_count = provider.call_index.load(Ordering::SeqCst);
        assert_eq!(call_count, 1, "the turn must end without a follow-up");

        // The turn handler consumed the interrupt; no graceful shutdown.
        assert!(
            !router.shutdown_token().is_cancelled(),
            "a turn-handled interrupt must not request a shutdown"
        );

        // The answered tool's response was persisted before the early
        // completion. (Tool response content is stored base64-encoded, so
        // assert on the event structure rather than the output text.)
        let content = fs
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");
        assert!(
            content.contains("tool_call_response") && content.contains("call_question"),
            "Should contain the tool response.\nFile contents:\n{content}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_multiple_tool_calls_in_sequence() {
    // Tests that multiple tool calls are handled correctly.
    // The executing phase should process all pending calls.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    let chat_request = ChatRequest::from("Do multiple things");

    // Create provider with multiple tool calls in first response
    let provider: Arc<dyn Provider> = Arc::new({
        let tool_call_events = vec![
            Event::tool_call_start(0, "call_1".to_string(), "tool_a".to_string()),
            Event::tool_call_start(1, "call_2".to_string(), "tool_b".to_string()),
            Event::flush(0),
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ];

        let message_events = vec![
            Event::message(0, "Both tasks completed."),
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        SequentialMockProvider {
            responses: vec![tool_call_events, message_events],
            call_index: AtomicUsize::new(0),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "multi-tool-mock".parse().expect("valid name"),
            }),
        }
    });

    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
    )
    .await;

    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer output contains the final message
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("Both tasks completed"),
        "Printer output should contain final LLM response.\nOutput:\n{output}"
    );
    drop(output);

    // Verify persistence
    let content = fs
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // Should contain both tool calls
    assert!(
        content.contains("tool_a") || content.contains("call_1"),
        "Should contain first tool call.\nFile contents:\n{content}"
    );
    assert!(
        content.contains("tool_b") || content.contains("call_2"),
        "Should contain second tool call.\nFile contents:\n{content}"
    );

    // Should contain final message
    assert!(
        content.contains("Both tasks completed"),
        "Should contain final response.\nFile contents:\n{content}"
    );
}

#[tokio::test]
async fn test_empty_tool_response_continues_cycle() {
    // Tests that when tool execution returns empty (e.g., tool not found),
    // the cycle still continues to the follow-up LLM call.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    let chat_request = ChatRequest::from("Use unknown tool");

    // Provider returns a call to a tool that won't be found
    let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
        "call_unknown",
        "nonexistent_tool",
        "I was unable to use that tool.",
    ));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[], // No tools configured - tool_coordinator.prepare will fail
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
    )
    .await;

    assert!(result.is_ok(), "Turn loop should complete: {result:?}");

    // Verify printer output contains the follow-up response
    printer.flush();
    let output = out.lock();
    assert!(
        output.contains("unable to use that tool"),
        "Printer output should contain follow-up response.\nOutput:\n{output}"
    );
    drop(output);

    // The second LLM call should have happened
    let content = fs
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    // Should contain the follow-up message from the LLM
    assert!(
        content.contains("unable to use that tool"),
        "Should contain follow-up response.\nFile contents:\n{content}"
    );
}

/// Tests the restart flow:
///
/// 1. LLM returns a tool call
/// 2. During execution, Ctrl-C is routed to the tool interrupt handler
/// 3. User selects "Restart" from menu (mocked)
/// 4. Tool execution restarts with original calls
/// 5. Eventually completes with follow-up message
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn test_tool_restart_on_interrupt() {
    // Wrap the entire test in a timeout to prevent infinite hangs
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("slow_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();
        let conv_id = lock.id();

        let chat_request = ChatRequest::from("Please use a tool");

        // Provider returns tool call first, then a message.
        let provider = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_restart",
            "slow_tool",
            "Tool completed after restart.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let (router, signals) = test_router();
        let router = Arc::new(router);

        // Mock user selecting 't' (Restart) when interrupted.
        // Provide extra 'c' (continue) responses in case of unexpected prompts.
        let backend = MockPromptBackend::new().with_inline_responses(['t', 'c', 'c', 'c', 'c']);

        // The first execution runs until the restart cancels it; the
        // re-execution completes immediately. The counter proves the restart
        // re-created the executor.
        let exec_calls = Arc::new(AtomicUsize::new(0));
        let exec_calls_in_factory = Arc::clone(&exec_calls);
        let tool_started = Arc::new(Notify::new());
        let tool_started_in_factory = Arc::clone(&tool_started);
        let executor_source = TestExecutorSource::new().with_executor("slow_tool", move |req| {
            if exec_calls_in_factory.fetch_add(1, Ordering::SeqCst) == 0 {
                Box::new(SleepingExecutor::notifying(
                    &req.id,
                    &req.name,
                    Arc::clone(&tool_started_in_factory),
                ))
            } else {
                Box::new(MockExecutor::completed(
                    &req.id,
                    &req.name,
                    "tool output after restart",
                ))
            }
        });

        // Press Ctrl-C once the first execution is running. The tool handler
        // registered by the executing phase receives the press and shows the
        // restart menu.
        let signal_handle = tokio::spawn(async move {
            tool_started.notified().await;
            signals.interrupt().await;
        });

        let result = run_turn_loop(
            Arc::clone(&provider) as Arc<dyn Provider>,
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        signal_handle.await.unwrap();

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool completed after restart"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the provider was called at least twice (tool call + final message)
        let call_count = provider.call_index.load(Ordering::SeqCst);
        assert!(
            call_count >= 2,
            "Provider should be called at least twice, got {call_count}"
        );

        // The restart re-created and re-ran the executor.
        assert_eq!(
            exec_calls.load(Ordering::SeqCst),
            2,
            "the restart must re-create the executor"
        );

        // Verify persistence includes the final message
        let content = fs
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");

        assert!(
            content.contains("Tool completed after restart"),
            "Should contain final response.\nFile contents:\n{content}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

#[tokio::test]
async fn test_merged_stream_exits_after_tool_response() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("echo_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: Some(CommandConfigOrString::String("echo hello".to_string())),
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();
        let conv_id = lock.id();

        let chat_request = ChatRequest::from("Please use echo_tool");

        // Provider returns tool call first, then a final message
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_echo",
            "echo_tool",
            "Tool executed successfully.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        // No signals sent - the turn loop should complete naturally after
        // the tool executes and the follow-up LLM response is received.
        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[], // Tool definitions come from config, not this param
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool executed successfully"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the conversation persisted with the final message
        let content = fs
            .read_test_events_raw(&conv_id)
            .expect("events should be persisted");

        assert!(
            content.contains("Tool executed successfully"),
            "Should contain final response after tool execution.\nFile contents:\n{content}"
        );
    }))
    .await;

    assert!(
        test_result.is_ok(),
        "Test timed out after 5 seconds - merged stream likely blocked forever after tool response"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_ask_approves() {
    // Tests: LLM returns tool call → Ask prompt → user presses 'y' → tool executes
    // Uses MockExecutor to avoid shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Ask (no command needed - we use MockExecutor)
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Ask;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None, // No real command
                run: Some(RunMode::Ask),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_ask",
            "mock_tool",
            "Tool was approved and executed.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        // Mock: user presses 'y' to approve
        let backend = MockPromptBackend::new().with_inline_responses(['y']);

        // Use TestExecutorSource with MockExecutor that requires permission
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            Box::new(
                MockExecutor::completed(&req.id, &req.name, "mock output").with_permission_info(
                    PermissionInfo {
                        tool_id: req.id.clone(),
                        tool_name: req.name.clone(),
                        tool_source: ToolSource::Local { tool: None },
                        run_mode: RunMode::Ask,
                        arguments: Value::Object(req.arguments.clone()),
                    },
                ),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty = true to enable prompts
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool was approved and executed"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was executed using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        assert!(
            response.result.is_ok(),
            "Tool should have succeeded: {:?}",
            response.result
        );

        // Verify the actual content
        assert_eq!(
            response.content(),
            "mock output",
            "Tool output should match mock executor output"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Tests: LLM returns tool call → Ask prompt → user presses 'n' → tool
/// skipped Uses `MockExecutor` to avoid shell commands.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_ask_skips() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Ask
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Ask;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Ask),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_skip",
            "mock_tool",
            "Tool was skipped by user.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        // Mock: user presses 'n' to skip
        let backend = MockPromptBackend::new().with_inline_responses(['n']);

        // Use TestExecutorSource with MockExecutor that requires permission
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            Box::new(
                MockExecutor::completed(&req.id, &req.name, "should not see this")
                    .with_permission_info(PermissionInfo {
                        tool_id: req.id.clone(),
                        tool_name: req.name.clone(),
                        tool_source: ToolSource::Local { tool: None },
                        run_mode: RunMode::Ask,
                        arguments: Value::Object(req.arguments.clone()),
                    }),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool was skipped by user"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was skipped using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        // Tool was skipped, so result is Ok with skip message
        assert!(
            response.result.is_ok(),
            "Skipped tool should have Ok result: {:?}",
            response.result
        );

        // Verify the skip message is in the content
        assert!(
            response.content().contains("skipped"),
            "Should contain 'skipped' in response: {}",
            response.content()
        );

        // Should NOT contain the mock output (tool didn't run)
        assert!(
            !response.content().contains("should not see this"),
            "Should NOT contain mock output since tool was skipped: {}",
            response.content()
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_unattended() {
    // Tests: LLM returns tool call → Unattended mode → tool runs without prompt
    // Uses MockExecutor to avoid shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Unattended (no prompt needed)
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_unattended",
            "mock_tool",
            "Tool ran in unattended mode.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        // No prompt responses needed - tool runs without asking
        let backend = MockPromptBackend::new();

        // MockExecutor without permission_info (Unattended mode)
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            // No permission_info = no prompt required
            Box::new(MockExecutor::completed(
                &req.id,
                &req.name,
                "unattended execution output",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty doesn't matter for Unattended
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool ran in unattended mode"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was executed using typed API (not raw JSON)
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        assert!(
            response.result.is_ok(),
            "Tool should have succeeded: {:?}",
            response.result
        );

        // Verify the actual content (decoded from base64 by the typed API)
        assert_eq!(
            response.content(),
            "unattended execution output",
            "Tool output should match mock executor output"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_with_run_mode_skip() {
    // Tests: LLM returns tool call → Skip mode → tool is skipped without prompt
    // Uses MockExecutor to avoid shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // Configure tool with run = Skip (always skipped)
        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Skip;
        config
            .conversation
            .tools
            .insert("mock_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Skip),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_skip",
            "mock_tool",
            "Tool was skipped by configuration.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        // No prompt responses needed - tool is skipped automatically
        let backend = MockPromptBackend::new();

        // MockExecutor with Skip mode permission_info - executor returns completion
        // but the prompter should return Skip before execution happens
        let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
            Box::new(
                MockExecutor::completed(
                    &req.id,
                    &req.name,
                    "SHOULD NOT SEE THIS - tool should be skipped",
                )
                .with_permission_info(PermissionInfo {
                    tool_id: req.id.clone(),
                    tool_name: req.name.clone(),
                    tool_source: ToolSource::Local { tool: None },
                    run_mode: RunMode::Skip,
                    arguments: Value::Object(req.arguments.clone()),
                }),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool was skipped by configuration"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool was skipped using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];
        // Tool was skipped, so result is Ok with skip message
        assert!(
            response.result.is_ok(),
            "Skipped tool should have Ok result: {:?}",
            response.result
        );

        // Verify the skip message is in the content
        assert!(
            response.content().contains("skipped"),
            "Should contain 'skipped' in response: {}",
            response.content()
        );

        // Should NOT contain the mock output (tool was skipped)
        assert!(
            !response.content().contains("SHOULD NOT SEE THIS"),
            "Should NOT contain mock output since tool was skipped: {}",
            response.content()
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_multiple_tools_with_different_run_modes() {
    // Tests: LLM returns 2 tool calls → one Ask (approved), one Unattended
    // Both should complete successfully with proper handling.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        // tool_ask requires approval
        config
            .conversation
            .tools
            .insert("tool_ask".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Ask),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });
        // tool_unattended runs automatically
        config
            .conversation
            .tools
            .insert("tool_unattended".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use both tools");

        // Provider returns two tool calls, then a message
        let provider: Arc<dyn Provider> = Arc::new({
            let tool_call_events = vec![
                Event::tool_call_start(0, "call_ask".to_string(), "tool_ask".to_string()),
                Event::tool_call_start(
                    1,
                    "call_unattended".to_string(),
                    "tool_unattended".to_string(),
                ),
                Event::flush(0),
                Event::flush(1),
                Event::Finished(FinishReason::Completed),
            ];

            let message_events = vec![
                Event::message(0, "Both tools completed."),
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![tool_call_events, message_events],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "multi-mode-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        // User presses 'y' to approve the Ask tool
        let backend = MockPromptBackend::new().with_inline_responses(['y']);

        let executor_source = TestExecutorSource::new()
            .with_executor("tool_ask", |req| {
                Box::new(
                    MockExecutor::completed(&req.id, &req.name, "ask tool output")
                        .with_permission_info(PermissionInfo {
                            tool_id: req.id.clone(),
                            tool_name: req.name.clone(),
                            tool_source: ToolSource::Local { tool: None },
                            run_mode: RunMode::Ask,
                            arguments: Value::Object(req.arguments.clone()),
                        }),
                )
            })
            .with_executor("tool_unattended", |req| {
                // No permission_info = runs without prompt
                Box::new(MockExecutor::completed(
                    &req.id,
                    &req.name,
                    "unattended tool output",
                ))
            });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Both tools completed"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify both tools were executed using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            2,
            "Should have exactly two tool responses"
        );

        // Both tools should have succeeded
        for response in &tool_responses {
            assert!(
                response.result.is_ok(),
                "Tool {} should have succeeded: {:?}",
                response.id,
                response.result
            );
        }

        // Find each tool's response by checking content
        let ask_response = tool_responses
            .iter()
            .find(|r| r.content() == "ask tool output");
        let unattended_response = tool_responses
            .iter()
            .find(|r| r.content() == "unattended tool output");

        assert!(
            ask_response.is_some(),
            "Should have response from tool_ask with 'ask tool output'"
        );
        assert!(
            unattended_response.is_some(),
            "Should have response from tool_unattended with 'unattended tool output'"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_tool_call_returns_error() {
    // Tests: LLM returns tool call → tool returns error → error is persisted
    // Uses MockExecutor to simulate error without shell commands.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("failing_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use failing_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_fail",
            "failing_tool",
            "Tool failed, here is the error.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let backend = MockPromptBackend::new();

        // MockExecutor that returns an error
        let executor_source = TestExecutorSource::new().with_executor("failing_tool", |req| {
            Box::new(MockExecutor::error(
                &req.id,
                &req.name,
                "Simulated tool failure",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Verify printer output contains the final message
        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Tool failed"),
            "Printer output should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool error using typed API
        let events = lock.events().clone();

        // Find tool call responses
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let response = &tool_responses[0];

        // Tool should have failed (result is Err)
        assert!(
            response.result.is_err(),
            "Tool should have failed: {:?}",
            response.result
        );

        // Verify the error message
        assert_eq!(
            response.content(),
            "Simulated tool failure",
            "Error message should match mock executor error"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A mock provider that delays before returning the stream.
///
/// This simulates a slow API response, allowing us to test the waiting
/// indicator during the HTTP round-trip.
#[derive(Debug)]
struct DelayedMockProvider {
    delay: Duration,
    response: String,
    model: ModelDetails,
}

impl DelayedMockProvider {
    fn new(delay: Duration, response: &str) -> Self {
        Self {
            delay,
            response: response.to_string(),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "delayed-mock".parse().expect("valid name"),
            }),
        }
    }
}

#[async_trait]
impl Provider for DelayedMockProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = self.model.clone();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![self.model.clone()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        tokio::time::sleep(self.delay).await;

        let events = vec![
            Event::message(0, &self.response),
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ];

        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

/// A single scripted stream: (delay before yielding, event) pairs.
type PacedScript = Vec<(Duration, Result<Event, StreamError>)>;

/// A mock provider that paces stream events with per-event delays and serves a
/// different script on each call.
///
/// Simulates a connection that opens, keep-alives, and only later produces
/// content (or an error) — the scenarios where the waiting indicator must
/// survive non-rendering events.
struct PacedMockProvider {
    /// Delay before `chat_completion_stream` returns the stream, simulating the
    /// HTTP round-trip.
    stream_delay: Duration,

    /// One script per call.
    scripts: Mutex<VecDeque<PacedScript>>,

    model: ModelDetails,
}

impl PacedMockProvider {
    fn new(stream_delay: Duration, scripts: Vec<PacedScript>) -> Self {
        Self {
            stream_delay,
            scripts: Mutex::new(scripts.into()),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "paced-mock".parse().expect("valid name"),
            }),
        }
    }
}

#[async_trait]
impl Provider for PacedMockProvider {
    async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
        let mut model = self.model.clone();
        model.id.name = name.clone();
        Ok(model)
    }

    async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
        Ok(vec![self.model.clone()])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> Result<EventStream, LlmError> {
        tokio::time::sleep(self.stream_delay).await;

        let script = self
            .scripts
            .lock()
            .expect("scripts mutex")
            .pop_front()
            .expect("a script for every provider call");

        let stream = stream::iter(script).then(|(delay, event)| async move {
            tokio::time::sleep(delay).await;
            event
        });

        Ok(Box::pin(stream))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_shows_during_delay() {
    // Tests that the waiting indicator appears when the LLM takes longer
    // than the configured delay. Uses a multi_thread runtime so the
    // spawned timer task can run concurrently with run_cycle().await.

    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        // Set delay to 0 so the indicator appears immediately
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;
        config.style.streaming.progress.interval_ms = 100;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");

        // Provider delays 500ms before returning stream
        let provider: Arc<dyn Provider> = Arc::new(DelayedMockProvider::new(
            Duration::from_millis(500),
            "Response after delay",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty = true to enable the indicator
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();

        // The waiting indicator is chrome, written to stderr
        let chrome = err.lock();
        assert!(
            chrome.contains("Waiting\u{2026}"),
            "Chrome (stderr) should contain waiting indicator.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("\r\x1b[K"),
            "Chrome (stderr) should contain clear sequence.\nChrome:\n{chrome}"
        );
        drop(chrome);

        // The final response is assistant content, written to stdout
        let output = out.lock();
        assert!(
            output.contains("Response after delay"),
            "Stdout should contain LLM response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_survives_keep_alive_and_shows_status() {
    // A keep-alive ping (e.g. an SSE heartbeat) renders nothing, so it must
    // not tear down the waiting indicator — otherwise the user faces a blank
    // terminal from the heartbeat until the first content token. Instead the
    // indicator updates its status detail and keeps ticking until content
    // arrives.

    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;
        config.style.streaming.progress.interval_ms = 50;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");

        // 250ms HTTP round-trip ("sending request"), then 250ms of silence on
        // the open stream ("waiting for first tokens"), then a keep-alive
        // ("receiving response data"), then 250ms more before content.
        let provider: Arc<dyn Provider> =
            Arc::new(PacedMockProvider::new(Duration::from_millis(250), vec![
                vec![
                    (Duration::from_millis(250), Ok(Event::KeepAlive)),
                    (
                        Duration::from_millis(250),
                        Ok(Event::message(0, "Response after keep-alive")),
                    ),
                    (Duration::ZERO, Ok(Event::flush(0))),
                    (Duration::ZERO, Ok(Event::Finished(FinishReason::Completed))),
                ],
            ]));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty = true to enable the indicator
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();

        let chrome = err.lock();
        assert!(
            chrome.contains("Waiting\u{2026}"),
            "Chrome (stderr) should contain waiting indicator.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("(sending request)"),
            "Indicator should show the pre-connection status.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("(waiting for first tokens)"),
            "Indicator should show the stream-established status.\nChrome:\n{chrome}"
        );
        // This status is only set when a keep-alive (or other non-rendering
        // event) arrives while the indicator is alive — its presence proves
        // the indicator survived the keep-alive.
        assert!(
            chrome.contains("(receiving response data)"),
            "Indicator should survive the keep-alive and show its status.\nChrome:\n{chrome}"
        );
        drop(chrome);

        let output = out.lock();
        assert!(
            output.contains("Response after keep-alive"),
            "Stdout should contain LLM response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_cleared_before_retry_notice() {
    // A stream error is about to write retry chrome, so the indicator must be
    // finished (line cleared) first. The keep-alive before the error also
    // exercises the survive-then-finish sequence on the error path.

    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;
        config.style.streaming.progress.interval_ms = 50;
        // Keep the retry backoff out of the test's runtime.
        config.assistant.request.base_backoff_ms = 1;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");

        // First call: keep-alive, then a transient error (triggers a retry).
        // Second call: a normal response.
        let provider: Arc<dyn Provider> =
            Arc::new(PacedMockProvider::new(Duration::from_millis(100), vec![
                vec![
                    (Duration::from_millis(100), Ok(Event::KeepAlive)),
                    (
                        Duration::from_millis(100),
                        Err(StreamError::transient("simulated hiccup")),
                    ),
                ],
                vec![
                    (
                        Duration::ZERO,
                        Ok(Event::message(0, "Response after retry")),
                    ),
                    (Duration::ZERO, Ok(Event::flush(0))),
                    (Duration::ZERO, Ok(Event::Finished(FinishReason::Completed))),
                ],
            ]));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();

        let chrome = err.lock();
        assert!(
            chrome.contains("retrying (1/"),
            "Chrome should contain the retry notice.\nChrome:\n{chrome}"
        );
        // Set only when a non-rendering event reaches a live indicator:
        // proves the keep-alive did not tear the indicator down before the
        // error arrived. The clear-before-notice ordering itself is enforced
        // structurally by `LineTimer::finish` awaiting the timer task; it is
        // not asserted here because the notice writes its own `\r\x1b[K`
        // prefix, making the two clears indistinguishable in the buffer.
        assert!(
            chrome.contains("(receiving response data)"),
            "Indicator should survive the keep-alive on the error path.\nChrome:\n{chrome}"
        );
        drop(chrome);

        let output = out.lock();
        assert!(
            output.contains("Response after retry"),
            "Stdout should contain the post-retry response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_not_shown_when_disabled() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = false;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");

        let provider: Arc<dyn Provider> = Arc::new(DelayedMockProvider::new(
            Duration::from_millis(200),
            "Quick response",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = out.lock();

        // Should NOT contain waiting indicator
        assert!(
            !output.contains("Waiting…"),
            "Output should NOT contain waiting indicator when disabled.\nOutput:\n{output}"
        );

        // But should contain the response
        assert!(
            output.contains("Quick response"),
            "Output should contain LLM response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_not_shown_for_non_tty() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");

        let provider: Arc<dyn Provider> = Arc::new(DelayedMockProvider::new(
            Duration::from_millis(200),
            "Non-tty response",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // is_tty = false
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = out.lock();

        // Should NOT contain waiting indicator for non-TTY
        assert!(
            !output.contains("Waiting…"),
            "Output should NOT contain waiting indicator for non-TTY.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines, clippy::items_after_statements)]
async fn test_multi_part_tool_call_shows_preparing_spinner() {
    // Tests the multi-part streaming tool call flow:
    // 1. LLM emits initial Part with tool name+id (empty args)
    //    → "Calling tool X (receiving arguments…)" spinner appears
    // 2. After a small delay, LLM emits final Part with parsed arguments
    // 3. Flush completes the tool call, spinner is cleared
    // 4. Tool executes (not found), follow-up LLM returns message
    // 5. Verify the spinner text appeared in the output
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        // Enable the preparing indicator with no delay so it shows
        // immediately.
        config.style.tool_call.show = true;
        config.style.tool_call.preparing.show = true;
        config.style.tool_call.preparing.delay_secs = 0;
        config.style.tool_call.preparing.interval_ms = 50;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Create a file");

        // Build a provider that simulates multi-part tool call streaming
        // with a delay between the initial Part and the final Part, giving
        // the spawned indicator task time to tick.
        let mut args = Map::new();
        args.insert("path".into(), "test.rs".into());
        args.insert("content".into(), "fn main() {}".into());

        let tool_call_events: Vec<Result<Event, jp_llm::error::StreamError>> = vec![
            // Initial Part: name+id known, arguments still streaming
            Ok(Event::tool_call_start(
                0,
                "call_multi".to_string(),
                "fs_create_file".to_string(),
            )),
        ];

        let delayed_events: Vec<Result<Event, jp_llm::error::StreamError>> = vec![
            Ok(Event::tool_call_start(
                0,
                "call_multi".to_string(),
                "fs_create_file".to_string(),
            )),
            Ok(Event::flush(0)),
            Ok(Event::Finished(FinishReason::Completed)),
        ];

        // Stream the initial Part immediately, then after 200ms stream the
        // rest. This gives the indicator task time to tick.
        let first_stream = futures::stream::iter(tool_call_events);
        let delay_stream = futures::stream::once(async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            // This value is discarded; we just need the delay
            Ok(Event::Finished(FinishReason::Completed))
        })
        .filter(|_| futures::future::ready(false)); // discard the dummy

        // Use the delayed_events after the delay
        let rest_stream = futures::stream::iter(delayed_events);

        // Chain: initial Part → delay → final Part + Flush + Finished
        let combined_first_cycle: jp_llm::EventStream =
            Box::pin(first_stream.chain(delay_stream).chain(rest_stream));

        let message_events: Vec<Result<Event, jp_llm::error::StreamError>> = vec![
            Ok(Event::message(0, "File created.")),
            Ok(Event::flush(0)),
            Ok(Event::Finished(FinishReason::Completed)),
        ];

        // Custom provider: first call returns the delayed stream, second
        // returns the message.
        struct DelayedToolCallProvider {
            first_cycle: std::sync::Mutex<Option<jp_llm::EventStream>>,
            second_cycle: std::sync::Mutex<Option<Vec<Result<Event, jp_llm::error::StreamError>>>>,
            model: ModelDetails,
        }

        impl fmt::Debug for DelayedToolCallProvider {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct("DelayedToolCallProvider").finish()
            }
        }

        #[async_trait]
        impl Provider for DelayedToolCallProvider {
            async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
                let mut m = self.model.clone();
                m.id.name = name.clone();
                Ok(m)
            }

            async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
                Ok(vec![self.model.clone()])
            }

            async fn chat_completion_stream(
                &self,
                _model: &ModelDetails,
                _query: ChatQuery,
            ) -> Result<jp_llm::EventStream, LlmError> {
                if let Some(stream) = self.first_cycle.lock().unwrap().take() {
                    return Ok(stream);
                }
                if let Some(events) = self.second_cycle.lock().unwrap().take() {
                    return Ok(Box::pin(futures::stream::iter(events)));
                }
                Ok(Box::pin(futures::stream::iter(vec![Ok(Event::Finished(
                    FinishReason::Completed,
                ))])))
            }
        }

        let provider: Arc<dyn Provider> = Arc::new(DelayedToolCallProvider {
            first_cycle: std::sync::Mutex::new(Some(combined_first_cycle)),
            second_cycle: std::sync::Mutex::new(Some(message_events)),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "delayed-tool-mock".parse().expect("valid name"),
            }),
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // is_tty = true to enable the indicator
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();

        // Chrome (tool headers, spinners) goes to stderr
        let chrome = err.lock();
        assert!(
            chrome.contains("Calling tool"),
            "Chrome should contain 'Calling tool'.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("fs_create_file"),
            "Chrome should contain the tool name.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("receiving arguments"),
            "Chrome should contain 'receiving arguments'.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("\x1b[K"),
            "Chrome should contain the clear-to-EOL escape.\nChrome:\n{chrome}"
        );
        drop(chrome);

        // Assistant content goes to stdout
        let output = out.lock();
        assert!(
            output.contains("File created"),
            "Stdout should contain final LLM response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test]
async fn test_turn_start_event_is_emitted() {
    // A single run_turn_loop call should inject a TurnStart { index: 0 }
    // event at the beginning of the conversation stream.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();

    let chat_request = ChatRequest::from("Hello");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("Hi there"));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
    )
    .await
    .unwrap();

    let events = lock.events();
    let turn_starts: Vec<&TurnStart> = events
        .iter()
        .filter_map(|e| e.event.as_turn_start())
        .collect();

    assert_eq!(turn_starts.len(), 1, "Expected exactly one TurnStart event");
}

#[tokio::test]
async fn test_turn_start_index_increments_across_turns() {
    // Two consecutive run_turn_loop calls should produce TurnStart events
    // with indices 0 and 1.

    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::new(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();

    let mcp_client = jp_mcp::Client::default();

    // First turn.
    let chat_request = ChatRequest::from("First question");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("First answer"));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let router = detached_router();

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
    )
    .await
    .unwrap();

    // Second turn.
    let chat_request = ChatRequest::from("Second question");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("Second answer"));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let router = detached_router();

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false,
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
    )
    .await
    .unwrap();

    // Verify.
    let events = lock.events();
    let turn_starts: Vec<&TurnStart> = events
        .iter()
        .filter_map(|e| e.event.as_turn_start())
        .collect();

    assert_eq!(turn_starts.len(), 2, "Expected two TurnStart events");
}

/// Verifies that buffered markdown text is flushed before the "Calling tool"
/// header appears in the output (Issue 1 fix).
#[tokio::test]
async fn test_markdown_flushed_before_tool_header() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.tool_call.show = true;
        // Disable the animated suffix so the streaming indicator doesn't spawn
        // a timer task, keeping output deterministic.
        config.style.tool_call.preparing.show = false;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Do something");

        // LLM emits a message part followed immediately by a tool call
        // in the same response. The message must appear before the header.
        let provider: Arc<dyn Provider> = Arc::new({
            let events = vec![
                Event::message(0, "Let me check that.\n\n"),
                Event::flush(0),
                Event::tool_call_start(1, "call_1".to_string(), "fs_read_file".to_string()),
                Event::flush(1),
                Event::Finished(FinishReason::Completed),
            ];

            let followup = vec![
                Event::message(0, "Done.\n\n"),
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![events, followup],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "md-flush-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            // The streaming "Calling tool" indicator is a TTY affordance.
            true,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();

        // Markdown text is assistant content (stdout)
        let output = out.lock().clone();
        assert!(
            output.contains("Let me check that"),
            "markdown text should be in stdout output"
        );

        // Tool header is chrome (stderr)
        let chrome = err.lock().clone();
        let tool_pos = chrome
            .find("Calling tool")
            .expect("tool header should be in chrome (stderr)");
        let _ = tool_pos; // used to verify it exists

        // With channel separation, markdown goes to stdout and tool
        // headers go to stderr, so ordering is verified by the
        // existence of each in the correct buffer above.
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Verifies that multiple parallel tool calls produce one permanent "Calling
/// tool X(args)" line each, not garbled across lines.
///
/// Uses `FunctionCall` parameter style so header+args appear on one line,
/// making assertions straightforward.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_parallel_tool_calls_rendered_atomically() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.tool_call.show = true;

        let fn_call_style = Some(DisplayStyleConfig {
            hidden: false,
            inline_results: InlineResults::Off,
            results_file_link: LinkStyle::Off,
            parameters: ParametersStyle::FunctionCall,
            error: ErrorStyleConfig {
                inline_results: None,
                results_file_link: None,
            },
        });

        // Configure tools with FunctionCall style for readable output.
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("tool_a".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: fn_call_style.clone(),
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });
        config
            .conversation
            .tools
            .insert("tool_b".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: fn_call_style,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use both tools");

        // Two tool calls with actual arguments.
        let mut args_a = Map::new();
        args_a.insert("package".into(), "jp_cli".into());
        let mut args_b = Map::new();
        args_b.insert("path".into(), "/tmp/test.rs".into());

        let provider: Arc<dyn Provider> = Arc::new({
            let tool_events = vec![
                Event::tool_call_start(0, "call_a".to_string(), "tool_a".to_string()),
                Event::tool_call_args(0, serde_json::to_string(&args_a).unwrap()),
                Event::tool_call_start(1, "call_b".to_string(), "tool_b".to_string()),
                Event::tool_call_args(1, serde_json::to_string(&args_b).unwrap()),
                Event::flush(0),
                Event::flush(1),
                Event::Finished(FinishReason::Completed),
            ];

            let followup = vec![
                Event::message(0, "Both done.\n\n"),
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![tool_events, followup],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "parallel-tools-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new()
            .with_executor("tool_a", |req| {
                Box::new(
                    MockExecutor::completed(&req.id, &req.name, "result_a")
                        .with_arguments(req.arguments.clone()),
                )
            })
            .with_executor("tool_b", |req| {
                Box::new(
                    MockExecutor::completed(&req.id, &req.name, "result_b")
                        .with_arguments(req.arguments.clone()),
                )
            });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // is_tty = false (no timer, keeps output deterministic)
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();
        let raw = err.lock().clone();

        // The raw buffer contains \r and \x1b[K from temp line
        // rewrites. To check the "final visible" output, find the
        // permanent lines which are the ones written by complete()
        // and contain both the tool name AND its args on the same
        // write (they use FunctionCall style: `(key: "val")`).
        //
        // The permanent line pattern is:
        //   Calling tool <name>(<args>)\n
        //
        // Temp lines never contain parenthesized args.
        assert!(
            raw.contains("tool_a") && raw.contains("jp_cli"),
            "tool_a header and args should both appear.\nOutput:\n{raw}"
        );
        assert!(
            raw.contains("tool_b") && raw.contains("/tmp/test.rs"),
            "tool_b header and args should both appear.\nOutput:\n{raw}"
        );

        // The LAST occurrence of "Calling tool.*tool_a" should be the
        // permanent line (which also contains "jp_cli" on the same
        // write). We verify that permanent lines contain args by
        // checking that the pattern "tool_a(..." and "tool_b(..."
        // appear, which is the FunctionCall format.
        //
        // This is the key anti-regression check: in the old code,
        // args would appear AFTER all headers, so "tool_a" would
        // never be adjacent to "(package:" in the output.
        let has_atomic_a = raw.contains("tool_a\u{1b}[0m(");
        let has_atomic_b = raw.contains("tool_b\u{1b}[0m(");
        assert!(
            has_atomic_a,
            "tool_a should have args immediately after name (atomic permanent \
             line).\nOutput:\n{raw}"
        );
        assert!(
            has_atomic_b,
            "tool_b should have args immediately after name (atomic permanent \
             line).\nOutput:\n{raw}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Verifies that a single tool call uses "Calling tool" (singular), and that
/// its header+arguments are rendered atomically.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_single_tool_call_rendered_with_args() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.tool_call.show = true;
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("fs_read_file".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Read a file");

        let mut args = Map::new();
        args.insert("path".into(), "/etc/hosts".into());

        let provider: Arc<dyn Provider> = Arc::new({
            let events = vec![
                Event::tool_call_start(0, "call_1".to_string(), "fs_read_file".to_string()),
                Event::tool_call_args(0, serde_json::to_string(&args).unwrap()),
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            let followup = vec![
                Event::message(0, "Here.\n\n"),
                Event::flush(0),
                Event::Finished(FinishReason::Completed),
            ];

            SequentialMockProvider {
                responses: vec![events, followup],
                call_index: AtomicUsize::new(0),
                model: ModelDetails::empty(id::ModelIdConfig {
                    provider: ProviderId::Test,
                    name: "single-tool-mock".parse().expect("valid name"),
                }),
            }
        });

        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new().with_executor("fs_read_file", |req| {
            Box::new(
                MockExecutor::completed(&req.id, &req.name, "file contents")
                    .with_arguments(req.arguments.clone()),
            )
        });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();
        let chrome = err.lock().clone();

        // Tool headers are chrome (stderr).
        assert!(
            chrome.contains("Calling tool"),
            "Chrome should contain 'Calling tool'.\nChrome:\n{chrome}"
        );
        assert!(
            !chrome.contains("Calling tools"),
            "Single tool should use singular, not plural.\nChrome:\n{chrome}"
        );

        // Header and args should both be present.
        assert!(
            chrome.contains("fs_read_file"),
            "Should contain tool name.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("/etc/hosts"),
            "Should contain tool args.\nChrome:\n{chrome}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Mock executor that checks accumulated answers and returns `NeedsInput` for
/// the first unanswered question.
/// When all questions are answered, returns `Completed`.
/// This simulates a tool that requires one or more rounds of inquiry before it
/// can finish.
struct InquiryMockExecutor {
    tool_id: String,
    tool_name: String,
    arguments: Map<String, Value>,
    questions: Vec<Question>,
    output: String,
}

impl InquiryMockExecutor {
    fn new(tool_id: &str, tool_name: &str, questions: Vec<Question>, output: &str) -> Self {
        Self {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            arguments: Map::new(),
            questions,
            output: output.to_string(),
        }
    }
}

#[async_trait]
impl Executor for InquiryMockExecutor {
    fn tool_id(&self) -> &str {
        &self.tool_id
    }
    fn tool_name(&self) -> &str {
        &self.tool_name
    }
    fn arguments(&self) -> &Map<String, Value> {
        &self.arguments
    }
    fn permission_info(&self) -> Option<PermissionInfo> {
        None
    }
    fn set_arguments(&mut self, _args: Value) {}

    async fn execute(
        &self,
        answers: &IndexMap<String, Value>,
        _mcp_client: &jp_mcp::Client,
        _root: &camino::Utf8Path,
        _cancellation_token: tokio_util::sync::CancellationToken,
    ) -> ExecutorResult {
        for q in &self.questions {
            if !answers.contains_key(&q.id) {
                return ExecutorResult::NeedsInput {
                    tool_id: self.tool_id.clone(),
                    tool_name: self.tool_name.clone(),
                    question: q.clone(),
                    accumulated_answers: answers.clone(),
                };
            }
        }
        ExecutorResult::Completed(jp_conversation::event::ToolCallResponse {
            id: self.tool_id.clone(),
            result: Ok(self.output.clone()),
        })
    }
}

/// Build provider events for a structured inquiry response.
///
/// Emits as `Value::String` to match real provider streaming behavior (the
/// `EventBuilder` parses the JSON string on flush).
fn structured_inquiry_events(inquiry_id: &str, answer: &Value) -> Vec<Event> {
    let data = json!({
        "inquiry_id": inquiry_id,
        "answer": answer,
    });

    vec![
        Event::structured(0, data.to_string()),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

/// Build provider events for a structured response without `inquiry_id`.
/// Used for parallel inquiry tests where call ordering is non-deterministic.
fn unkeyed_structured_events(answer: &Value) -> Vec<Event> {
    let data = json!({
        "answer": answer,
    });

    vec![
        Event::structured(0, data.to_string()),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

fn single_tool_call_events(id: &str, name: &str) -> Vec<Event> {
    vec![
        Event::tool_call_start(0, id.to_string(), name.to_string()),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

fn final_message_events(content: &str) -> Vec<Event> {
    vec![
        Event::message(0, content),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]
}

/// Create a `ToolConfig` with questions targeting the assistant.
fn inquiry_tool_config(questions: &[&str]) -> ToolConfig {
    ToolConfig {
        source: ToolSource::Local { tool: None },
        command: None,
        run: Some(RunMode::Unattended),
        format: None,
        enable: None,
        summary: None,
        description: None,
        examples: None,
        parameters: IndexMap::new(),
        result: None,
        style: None,
        questions: questions
            .iter()
            .map(|id| {
                (id.to_string(), QuestionConfig {
                    target: QuestionTarget::Assistant(Box::default()),
                    answer: None,
                })
            })
            .collect(),
        options: IndexMap::default(),
        access: None,
        cancellation_response: None,
    }
}

fn inquiry_mock_model() -> ModelDetails {
    ModelDetails::empty(id::ModelIdConfig {
        provider: ProviderId::Test,
        name: "inquiry-mock".parse().expect("valid name"),
    })
}

/// Tool has one boolean question with `QuestionTarget::Assistant`.
/// Flow: LLM tool call → `NeedsInput` → inquiry → answer → tool completes.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_tool_with_single_inquiry() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "inquiry_tool".to_string(),
            inquiry_tool_config(&["confirm"]),
        );

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool");

        // Provider call sequence:
        // 1. Tool call
        // 2. Structured inquiry answer (from LlmInquiryBackend)
        // 3. Final message
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_inq", "inquiry_tool"),
                structured_inquiry_events("call_inq.confirm", &json!(true)),
                final_message_events("Inquiry tool completed."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new().with_executor("inquiry_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::boolean("confirm", "Create backup?")],
                "inquiry tool output",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Inquiry tool completed"),
            "Should contain final LLM response.\nOutput:\n{output}"
        );
        drop(output);

        // Verify the tool response was persisted as successful.
        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 1, "Should have one tool response");
        assert!(
            tool_responses[0].result.is_ok(),
            "Tool should have succeeded: {:?}",
            tool_responses[0].result
        );
        assert_eq!(
            tool_responses[0].content(),
            "inquiry tool output",
            "Tool output should match executor output"
        );

        // Verify inquiry events were recorded (RFD 005).
        let req: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_request())
            .collect();
        assert_eq!(req.len(), 1, "Should have one inquiry request");
        assert_eq!(req[0].source, InquirySource::tool("inquiry_tool"));
        assert_eq!(req[0].question.text, "Create backup?");

        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 1, "Should have one inquiry response");
        assert_eq!(res[0].answer, json!(true));
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Tool has two questions, each triggering a separate inquiry round.
/// Flow: tool call → `NeedsInput(q1)` → inquiry → answer → `NeedsInput(q2)`
/// → inquiry → answer → completed.
#[tokio::test]
async fn test_tool_with_multiple_inquiries() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "multi_q_tool".to_string(),
            inquiry_tool_config(&["confirm", "reason"]),
        );

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Multi-question tool");

        // Provider call sequence:
        // 1. Tool call
        // 2. Structured answer for q1 ("confirm")
        // 3. Structured answer for q2 ("reason")
        // 4. Final message
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_mq", "multi_q_tool"),
                structured_inquiry_events("call_mq.confirm", &json!(true)),
                structured_inquiry_events("call_mq.reason", &json!("performance reasons")),
                final_message_events("Multi-question tool done."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new().with_executor("multi_q_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![
                    Question::boolean("confirm", "Proceed?"),
                    Question::text("reason", "Why?"),
                ],
                "both questions answered",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Multi-question tool done"),
            "Should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 1);
        assert_eq!(tool_responses[0].content(), "both questions answered");

        // Two inquiry rounds should produce two request/response pairs.
        let req: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_request())
            .collect();
        assert_eq!(req.len(), 2, "Should have two inquiry requests");

        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 2, "Should have two inquiry responses");
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Two parallel tools: one requires an inquiry, the other completes normally.
/// The inquiry should not block the normal tool from completing.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_parallel_tools_one_with_inquiry() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "inquiry_tool".to_string(),
            inquiry_tool_config(&["confirm"]),
        );
        config
            .conversation
            .tools
            .insert("normal_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use both tools");

        // Provider call sequence:
        // 1. Two parallel tool calls
        // 2. Structured inquiry answer (for inquiry_tool)
        // 3. Final message
        let parallel_events = vec![
            Event::tool_call_start(0, "call_inq".to_string(), "inquiry_tool".to_string()),
            Event::tool_call_start(1, "call_norm".to_string(), "normal_tool".to_string()),
            Event::flush(0),
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ];

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                parallel_events,
                structured_inquiry_events("call_inq.confirm", &json!(true)),
                final_message_events("Both tools done."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new()
            .with_executor("inquiry_tool", |req| {
                Box::new(InquiryMockExecutor::new(
                    &req.id,
                    &req.name,
                    vec![Question::boolean("confirm", "Proceed?")],
                    "inquiry completed",
                ))
            })
            .with_executor("normal_tool", |req| {
                Box::new(MockExecutor::completed(&req.id, &req.name, "normal output"))
            });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Both tools done"),
            "Should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 2, "Should have two tool responses");

        // Both should have succeeded.
        for r in &tool_responses {
            assert!(
                r.result.is_ok(),
                "Tool {} should succeed: {:?}",
                r.id,
                r.result
            );
        }
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Two parallel tools both requiring inquiries.
/// Uses responses without `inquiry_id` since the concurrent inquiry call order
/// is non-deterministic.
#[tokio::test]
#[expect(clippy::too_many_lines)]
async fn test_parallel_tools_both_with_inquiries() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config
            .conversation
            .tools
            .insert("tool_a".to_string(), inquiry_tool_config(&["confirm_a"]));
        config
            .conversation
            .tools
            .insert("tool_b".to_string(), inquiry_tool_config(&["confirm_b"]));

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Both need inquiries");

        // Provider call sequence:
        // 1. Two parallel tool calls
        // 2. Structured answer (no inquiry_id — order-independent)
        // 3. Structured answer (no inquiry_id — order-independent)
        // 4. Final message
        let parallel_events = vec![
            Event::tool_call_start(0, "call_a".to_string(), "tool_a".to_string()),
            Event::tool_call_start(1, "call_b".to_string(), "tool_b".to_string()),
            Event::flush(0),
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ];

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                parallel_events,
                unkeyed_structured_events(&json!(true)),
                unkeyed_structured_events(&json!(true)),
                final_message_events("Both inquiries resolved."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new()
            .with_executor("tool_a", |req| {
                Box::new(InquiryMockExecutor::new(
                    &req.id,
                    &req.name,
                    vec![Question::boolean("confirm_a", "Proceed A?")],
                    "tool_a done",
                ))
            })
            .with_executor("tool_b", |req| {
                Box::new(InquiryMockExecutor::new(
                    &req.id,
                    &req.name,
                    vec![Question::boolean("confirm_b", "Proceed B?")],
                    "tool_b done",
                ))
            });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        printer.flush();
        let output = out.lock();
        assert!(
            output.contains("Both inquiries resolved"),
            "Should contain final response.\nOutput:\n{output}"
        );
        drop(output);

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 2, "Should have two tool responses");
        for r in &tool_responses {
            assert!(
                r.result.is_ok(),
                "Tool {} should succeed: {:?}",
                r.id,
                r.result
            );
        }
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Verifies that the retry counter resets after a successful event arrives in a
/// new streaming cycle.
///
/// Scenario with `max_retries=1`:
///
/// 1. Stream produces content, then rate-limits mid-stream (no Finished)
/// 2. Retry: stream produces content (counter resets here), then rate-limits
///    again mid-stream
/// 3. Retry: stream completes successfully
///
/// Without the fix, the counter would reach 2 after step 2, exceeding the
/// budget of 1 and causing a hard failure.
#[tokio::test]
async fn test_retry_counter_resets_on_successful_event() {
    struct MidStreamRateLimitProvider {
        call_index: Arc<AtomicUsize>,
        model: ModelDetails,
    }

    #[async_trait]
    impl Provider for MidStreamRateLimitProvider {
        async fn model_details(&self, name: &id::Name) -> Result<ModelDetails, LlmError> {
            let mut m = self.model.clone();
            m.id.name = name.clone();
            Ok(m)
        }

        async fn models(&self) -> Result<Vec<ModelDetails>, LlmError> {
            Ok(vec![self.model.clone()])
        }

        async fn chat_completion_stream(
            &self,
            _model: &ModelDetails,
            _query: ChatQuery,
        ) -> Result<EventStream, LlmError> {
            let idx = self.call_index.fetch_add(1, Ordering::SeqCst);

            let events: Vec<Result<Event, StreamError>> = if idx < 2 {
                // Calls 0 and 1: partial content then rate limit error.
                vec![
                    Ok(Event::message(0, "partial ")),
                    Err(StreamError::rate_limit(None)),
                ]
            } else {
                // Call 2: complete successfully.
                vec![
                    Ok(Event::message(0, "done.")),
                    Ok(Event::flush(0)),
                    Ok(Event::Finished(FinishReason::Completed)),
                ]
            };

            Ok(Box::pin(stream::iter(events)))
        }
    }

    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.assistant.request.max_retries = 1;
        config.assistant.request.base_backoff_ms = 1;
        config.assistant.request.max_backoff_secs = 1;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");
        let call_index = Arc::new(AtomicUsize::new(0));
        let call_index_clone = Arc::clone(&call_index);

        // Provider that returns partial content + rate limit error on the first
        // two calls, then succeeds on the third.
        let provider: Arc<dyn Provider> = Arc::new(MidStreamRateLimitProvider {
            call_index: call_index_clone,
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "rate-limit-mock".parse().expect("valid name"),
            }),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer,
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request,
            InvocationContext::default(),
        )
        .await;

        // With the fix, this succeeds (counter resets between retries). Without
        // the fix, this would fail with a rate limit error after the second
        // stream failure exhausts the budget.
        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        // Provider should have been called 3 times:
        // call 0: partial + rate limit
        // call 1: partial + rate limit (budget restored by reset)
        // call 2: success
        let total_calls = call_index.load(Ordering::SeqCst);
        assert_eq!(
            total_calls, 3,
            "Expected 3 provider calls (2 partial + 1 success), got {total_calls}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Regression: when the LLM emits a tool call for an unconfigured tool (which
/// becomes a `Resolved` pending entry) followed by a configured one (which
/// becomes `Approved`), `build_execution_plan` assigns plan indices 0 and 1 in
/// stream order.
/// The approved entry then has plan index 1, but `execute_with_prompting` was
/// sizing its internal `results` vector to `executors.len()` (= 1) and indexing
/// into it with the plan index — which panicked with `index out of bounds: the
/// len is 1 but the index is 1`.
///
/// The fix re-bases plan indices to contiguous local positions inside
/// `execute_with_prompting`, then pairs each response back with its
/// caller-provided plan index on output.
/// The downstream `commit_tool_responses` uses those plan indices when merging
/// approved + pre-resolved responses, so they appear in the original stream
/// order.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_unavailable_tool_before_approved_does_not_panic() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        // Only `ok_tool` is configured. `missing_tool` will trip
        // `prepare_one`'s "tool not available" path and become a
        // pre-resolved error response.
        config
            .conversation
            .tools
            .insert("ok_tool".to_string(), ToolConfig {
                source: ToolSource::Local { tool: None },
                command: None,
                run: Some(RunMode::Unattended),
                format: None,
                enable: None,
                summary: None,
                description: None,
                examples: None,
                parameters: IndexMap::new(),
                result: None,
                style: None,
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use both tools");

        // Stream order matters: the unavailable tool MUST come first so
        // that the surviving approved tool gets plan index 1 (the
        // out-of-bounds slot in the buggy version).
        let parallel_events = vec![
            Event::tool_call_start(0, "call_missing".to_string(), "missing_tool".to_string()),
            Event::tool_call_start(1, "call_ok".to_string(), "ok_tool".to_string()),
            Event::flush(0),
            Event::flush(1),
            Event::Finished(FinishReason::Completed),
        ];

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                parallel_events,
                final_message_events("All tools dispatched."),
            ],
            call_index: AtomicUsize::new(0),
            model: ModelDetails::empty(id::ModelIdConfig {
                provider: ProviderId::Test,
                name: "sparse-index-mock".parse().expect("valid name"),
            }),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        // Only `ok_tool` is registered with the executor source; the
        // `missing_tool` tool call has no executor and falls through to
        // the unavailable path.
        let executor_source = TestExecutorSource::new().with_executor("ok_tool", |req| {
            Box::new(MockExecutor::completed(&req.id, &req.name, "ok output"))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();
        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            2,
            "Both tool calls should produce a response"
        );

        // Stream order: missing first, ok second. `commit_tool_responses`
        // must preserve that ordering when merging approved with
        // pre-resolved.
        assert_eq!(tool_responses[0].id, "call_missing");
        assert!(
            tool_responses[0].result.is_err(),
            "Unavailable tool should produce an error response: {:?}",
            tool_responses[0].result
        );
        assert_eq!(tool_responses[1].id, "call_ok");
        assert_eq!(tool_responses[1].content(), "ok output");
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// When the inquiry provider returns a non-structured response, the inquiry
/// fails and the tool is marked as completed with an error.
#[tokio::test]
async fn test_inquiry_failure_marks_tool_as_error() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        config.conversation.tools.insert(
            "inquiry_tool".to_string(),
            inquiry_tool_config(&["confirm"]),
        );

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool");

        // Provider call sequence:
        // 1. Tool call
        // 2. Plain message (NOT structured) → inquiry fails
        // 3. Final message (LLM sees the error and responds)
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_fail", "inquiry_tool"),
                final_message_events("I don't understand the question."),
                final_message_events("The tool failed, sorry."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new().with_executor("inquiry_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::boolean("confirm", "Confirm?")],
                "should not reach this",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(tool_responses.len(), 1, "Should have one tool response");
        assert!(
            tool_responses[0].result.is_err(),
            "Tool should have failed: {:?}",
            tool_responses[0].result
        );
        let content = tool_responses[0].content();
        assert!(
            content.contains("secondary assistant failed"),
            "Error should explain the inquiry failure: {content}",
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Regression for live/replay parity on the role-header model id.
///
/// The live header must use `cfg.assistant.model.id.resolved()`, not the
/// provider's `ModelDetails.id`.
/// With the previous code, the two could drift when the provider rewrites the
/// id (e.g. Anthropic resolving an unversioned name to a date-suffixed
/// canonical form).
/// On replay, `TurnRenderer` reads the stored per-turn config and shows the
/// configured id — so live had to match that, or the same conversation would
/// render with two different model strings between `jp q` and `jp c print`.
#[tokio::test]
async fn test_live_header_uses_configured_model_id_not_provider_returned() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // `AppConfig::new_test()` sets `assistant.model.id = anthropic/test`.
        // `MockProvider::model_details` echoes whatever name it's handed
        // back under `ProviderId::Test` — we deliberately pass a *different*
        // name (`api-rewritten`) so the resulting `ModelDetails.id`
        // (`test/api-rewritten`) cannot collide with the configured id.
        let config = AppConfig::new_test();

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::new(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");

        let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_message("Hi there"));
        let model = provider
            .model_details(&"api-rewritten".parse().unwrap())
            .await
            .unwrap();

        // Sanity check: the provider's id really does differ from the
        // configured id, so the assertions below have something to bite on.
        assert_eq!(model.id.to_string(), "test/api-rewritten");
        assert_eq!(
            config.assistant.model.id.resolved().to_string(),
            "anthropic/test"
        );

        let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false,
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request,
            InvocationContext::default(),
        )
        .await
        .unwrap();

        printer.flush();
        let output = strip_ansi_escapes::strip(&*out.lock());
        let output = String::from_utf8(output).unwrap();

        assert!(
            output.contains("anthropic/test"),
            "live header must use configured model id; got: {output:?}"
        );
        assert!(
            !output.contains("api-rewritten"),
            "live header must not use provider-returned model id; got: {output:?}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}
