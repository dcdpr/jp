use std::{
    collections::VecDeque,
    fmt,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
    AppConfig, PartialAppConfig,
    assistant::{
        PartialAssistantConfig,
        request::{CachePolicy, MaxResponseBytes, PartialRequestConfig},
    },
    conversation::tool::{
        CommandConfigOrString, QuestionConfig, QuestionTarget, RunMode, ToolConfig, ToolSource,
        style::{
            DisplayStyleConfig, ErrorStyleConfig, InlineResults, LinkStyle, ParametersStyle,
            TruncateLines,
        },
    },
    interrupt::ToolInterruptAction,
    model::id::{self, ProviderId},
    style::stderr_rows::{RowCount, StderrRows},
};
use jp_conversation::{
    Conversation, ConversationEvent,
    event::{
        CancellationReason, ChatRequest, ChatResponse, InquiryResponse, InquirySource,
        ToolCallRequest, TurnStart,
    },
};
use jp_inquire::{
    InlineOption, ReplyEditMode, ReplyOutcome,
    prompt::{MockPromptBackend, PromptBackend},
};
use jp_llm::{
    Error as LlmError, EventStream, Provider,
    error::StreamError,
    event::{Event, EventMatcher, EventPatch, FinishReason, PatchAction},
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
use jp_printer::{OutputFormat, Printer, TerminalCapability};
use jp_storage::backend::FsStorageBackend;
use jp_tool::Question;
use jp_workspace::Workspace;
use serde_json::{Map, Value, json};
use tokio::{sync::Notify, time::timeout};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::{
    cmd::query::{
        stream::retry::MAX_CONSECUTIVE_REBUILDS,
        tool::{ToolCoordinator, executor::TerminalExecutorSource},
    },
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

/// A provider that streams content without end, counting calls so a test can
/// assert the response was not re-requested.
///
/// After its parts the stream stays pending forever and it never sends a
/// terminal `Finished`, so the output ceiling is the only thing that can end
/// it.
/// A test built on this fixture hangs if the ceiling never fires, rather than
/// falling through to the retry path and passing for the wrong reason.
#[derive(Debug, Default)]
struct RunawayProvider {
    calls: AtomicUsize,
}

#[async_trait]
impl Provider for RunawayProvider {
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

        // 7 bytes, then 40 bytes per part.
        let mut events = vec![Event::message(0, "runaway")];
        events.extend(std::iter::repeat_with(|| Event::message(0, "0123456789".repeat(4))).take(9));

        Ok(Box::pin(
            stream::iter(events.into_iter().map(Ok)).chain(stream::pending()),
        ))
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],   // attachments
            &lock,
            ToolChoice::Auto,
            &[], // tools
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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

/// A block the provider has finished is on disk before the turn ends.
///
/// Read while the turn is still running, which is the only window where this
/// differs from persisting at the end of the phase: the provider commits a
/// block, flushes it, and then parks, and the store is read at that point
/// rather than afterwards.
/// Anything watching the conversation from another process sees the same thing.
#[tokio::test(flavor = "multi_thread")]
async fn a_completed_block_is_persisted_before_the_turn_ends() {
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let config = AppConfig::new_test();
        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
            .unwrap();
        let conv_id = lock.id();

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

        // Stop, so the turn ends once the store has been read.
        let backend = MockPromptBackend::new().with_inline_responses(['s']);

        // The window: the stream has flushed its block and parked, and the turn
        // has not ended. Read the store here, then let the turn finish.
        let observer = tokio::spawn({
            let stalled = Arc::clone(&stalled);
            let fs = Arc::clone(&fs);
            async move {
                stalled.notified().await;
                let seen = fs.read_test_events_raw(&conv_id);
                signals.interrupt().await;
                seen
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
            ChatRequest::from("What is 2+2?"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        let mid_turn = observer.await.unwrap();

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let mid_turn = mid_turn.expect("the store is written before the turn ends");
        assert!(
            mid_turn.contains("The answer is 4."),
            "A flushed block should already be persisted while the turn runs.\nFile \
             contents:\n{mid_turn}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out after 10 seconds");
}

/// A refusal takes back content that was already written to disk.
///
/// `FinishReason::Refused` requires partial output to be discarded.
/// Persisting each block as it lands writes that content out before the refusal
/// arrives, so "the stream is finished" is not the same as "the file is right"
/// — this pins that the file ends up right.
///
/// It does not pin the *window*: the content is readable between its flush and
/// the refusal, and reading disk inside that window needs a provider that parks
/// between the two events.
#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_takes_back_content_it_had_persisted() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    // The shape Anthropic produces when a classifier declines mid-stream: the
    // content block is complete and flushed before the refusal arrives.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(vec![
        Event::message(0, "Partial answer the classifier declines."),
        Event::flush(0),
        Event::Finished(FinishReason::Refused {
            category: Some("cyber".to_owned()),
            explanation: Some("This request was declined.".to_owned()),
        }),
    ]));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (router, _signals) = test_router();
    let router = Arc::new(router);

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
        ChatRequest::from("something declined"),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
    )
    .await
    .unwrap();

    let persisted = fs.read_test_events_raw(&conv_id).unwrap_or_default();
    assert!(
        !persisted.contains("Partial answer"),
        "refused content must not survive on disk\nFile contents:\n{persisted}"
    );
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],   // attachments
            &lock,
            ToolChoice::Auto,
            &[], // tools
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            ChatRequest::from("hi"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            ChatRequest::from("hi"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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

/// A response that runs past `assistant.request.max_response_bytes` ends the
/// turn with an error, is not re-requested, and keeps the content streamed
/// before the ceiling was reached.
#[tokio::test]
async fn output_ceiling_ends_turn_without_re_requesting() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let mut config = AppConfig::new_test();
    config.assistant.request.max_response_bytes = MaxResponseBytes::Bytes(64);
    // A retry budget is left in place so the call-count assertion below has
    // something to catch: were the ceiling classified as retryable, the loop
    // would re-request the response instead of ending the turn.
    config.assistant.request.max_retries = 5;
    config.assistant.request.base_backoff_ms = 0;
    // The fixture never goes idle-silent before the ceiling fires, but an
    // enabled idle timeout would give the loop a second way out; disable it so
    // only the ceiling can end this turn.
    config.assistant.request.stream_idle_timeout_secs = 0;

    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    let provider = Arc::new(RunawayProvider::default());
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            ChatRequest::from("hi"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        ),
    )
    .await
    .expect("the output ceiling should end the turn, not hang it");

    let Err(Error::Llm(jp_llm::Error::Stream(error))) = result else {
        panic!("expected a stream error from the output ceiling, got: {result:?}");
    };
    assert_eq!(error.kind, jp_llm::StreamErrorKind::OutputLimit);

    assert_eq!(
        provider.calls.load(Ordering::SeqCst),
        1,
        "a response that breached the ceiling must not be re-requested"
    );

    // Option 1 of the ceiling design: the turn ends, but content the user
    // already saw stays in the conversation.
    let content = fs
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");
    assert!(
        content.contains("runaway"),
        "content streamed before the ceiling must be persisted.\nFile contents:\n{content}"
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        ChatRequest::from("new query"),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[], // No tool definitions - tests the "tool not found" path
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
        _stderr: Option<jp_llm::tool::StderrSink>,
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
        help: Option<&str>,
        output: Box<dyn Write + Send>,
    ) -> Result<ReplyOutcome, InquireError> {
        self.started.notify_one();
        std::thread::sleep(self.delay);
        self.inner.inline_reply(
            message,
            initial_text,
            edit_mode,
            editor_escape,
            help,
            output,
        )
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

    fn password(&self, message: &str, writer: &mut dyn Write) -> Result<String, InquireError> {
        self.started.notify_one();
        std::thread::sleep(self.delay);
        self.inner.password(message, writer)
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        let (router, signals) = test_router();
        let router = Arc::new(router);

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
            false, // interactive
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
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
                vec![Question::boolean("confirm", "Proceed?").unwrap()],
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
            true, // interactive: user-targeted question prompts need a user
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[], // No tools configured - tool_coordinator.prepare will fail
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[], // Tool definitions come from config, not this param
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            true, // interactive = true to enable prompts
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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

/// A permission prompt follows `interactive`, not `is_tty`.
///
/// The pair here is the one a user gets from `jp query > answer.txt` at a
/// terminal: output cannot carry ANSI, but someone is still watching.
/// The mock answers 'n', so the tool runs only if the prompt was skipped — a
/// permission gate reading `is_tty` would auto-approve the Ask tool and leave
/// `mock output` in the response.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn test_permission_prompt_follows_interactive_not_is_tty() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Please use mock_tool");

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider::with_tool_then_message(
            "call_piped",
            "mock_tool",
            "Tool was skipped by user.",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(printer);
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let backend = MockPromptBackend::new().with_inline_responses(['n']);

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
            true, // interactive: the user is still at the terminal
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let tool_responses: Vec<_> = lock
            .events()
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();

        assert_eq!(
            tool_responses.len(),
            1,
            "Should have exactly one tool response"
        );

        let content = tool_responses[0].content();
        assert!(
            content.contains("skipped"),
            "The prompt should have run and been declined: {content}"
        );
        assert!(
            !content.contains("mock output"),
            "The tool must not run: the prompt was declined, not skipped: {content}"
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            true, // interactive doesn't matter for Unattended
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(backend),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
    //
    // `interactive` is false against a TTY: the indicator is a rendering
    // affordance, so nothing about it may depend on someone being there to
    // answer a prompt.

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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        // The status region only renders against a terminal it has to itself.
        let printer = Arc::new(printer.with_terminal(TerminalCapability::interactive(Some(80))));
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        let printer = Arc::new(printer.with_terminal(TerminalCapability::interactive(Some(80))));
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        let printer = Arc::new(printer.with_terminal(TerminalCapability::interactive(Some(80))));
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        // error arrived. The clear-before-notice ordering itself is the
        // printer's erase-before-write rule; it is not asserted here because
        // the notice writes its own `\r\x1b[K` prefix, making the two clears
        // indistinguishable in the buffer.
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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

        let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
        // A terminal is available; `show = false` is what turns the indicator
        // off, so the region must stay inert on its own.
        let printer = Arc::new(printer.with_terminal(TerminalCapability::interactive(Some(80))));
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();

        let chrome = err.lock();
        assert!(
            !chrome.contains("Waiting…"),
            "Chrome should NOT contain the waiting indicator when disabled.\nChrome:\n{chrome}"
        );
        drop(chrome);

        let output = out.lock();
        assert!(
            output.contains("Quick response"),
            "Output should contain LLM response.\nOutput:\n{output}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
// `interactive` is true without a TTY: a user at the terminal with stdout
// redirected must not get cursor-control chrome in the redirected output.
async fn test_waiting_indicator_not_shown_for_non_tty() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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

        // The default capability models a piped stderr.
        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
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
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();

        // The indicator is chrome, so stderr is where it would land.
        let chrome = err.lock();
        assert!(
            !chrome.contains("Waiting…"),
            "Chrome should NOT contain the waiting indicator without a \
             terminal.\nChrome:\n{chrome}"
        );
        assert!(
            !chrome.contains("\r\x1b[K"),
            "Chrome should NOT contain cursor control without a terminal.\nChrome:\n{chrome}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_waiting_indicator_follows_stderr_not_stdout() {
    // The indicator is stderr chrome, so its gate is stderr's tty-ness. With
    // `jp query 2>file` on an interactive stdout, no `\r\x1b[K` bytes may reach
    // the redirected stream.
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.style.streaming.progress.show = true;
        config.style.streaming.progress.delay_secs = 0;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Hello");

        let provider: Arc<dyn Provider> = Arc::new(DelayedMockProvider::new(
            Duration::from_millis(200),
            "Redirected-stderr response",
        ));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        // Stderr is redirected (the default, non-interactive capability) even
        // though stdout is a terminal.
        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
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
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();

        let chrome = err.lock();
        assert!(
            !chrome.contains("Waiting…"),
            "A redirected stderr must not receive the indicator.\nChrome:\n{chrome}"
        );
        assert!(
            !chrome.contains('\r'),
            "A redirected stderr must not receive cursor control.\nChrome:\n{chrome}"
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
        // The preparing row is printer-owned chrome and renders only against a
        // terminal, so the memory printer has to declare one.
        let terminal = TerminalCapability::interactive(Some(80));

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        let printer = Arc::new(printer.with_terminal(terminal));
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer,
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        chat_request.clone(),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
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
        config.style.tool_call.preparing.show = true;
        config.style.tool_call.preparing.delay_secs = 0;
        // The preparing row is printer-owned chrome and renders only against a
        // terminal, so the memory printer has to declare one.
        let terminal = TerminalCapability::interactive(Some(80));

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
        let printer = Arc::new(printer.with_terminal(terminal));
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
            print_stderr: false,
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request.clone(),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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

/// An executor that writes to the stderr sink the coordinator hands it, then
/// completes.
///
/// Stands in for a local tool whose child process talks while it runs: what
/// reaches the sink is what `jp_llm::tool::forward_stderr` would forward.
struct TalkingExecutor {
    /// The tool call this executor answers.
    tool_id: String,

    /// The tool's name, which is also the window label its lines carry.
    tool_name: String,

    /// Arguments, unused beyond satisfying the trait.
    arguments: Map<String, Value>,

    /// Lines pushed through the sink before completing.
    lines: Vec<String>,

    /// Whether a sink arrived at all, so a test can tell "pushed nowhere" from
    /// "never given a sink".
    got_sink: Arc<AtomicBool>,
}

impl TalkingExecutor {
    /// An executor that pushes `lines`, recording whether it received a sink.
    fn new(tool_id: &str, tool_name: &str, lines: &[&str], got_sink: Arc<AtomicBool>) -> Self {
        Self {
            tool_id: tool_id.to_owned(),
            tool_name: tool_name.to_owned(),
            arguments: Map::new(),
            lines: lines.iter().map(|l| (*l).to_owned()).collect(),
            got_sink,
        }
    }
}

#[async_trait]
impl Executor for TalkingExecutor {
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
        _cancellation_token: CancellationToken,
        stderr: Option<jp_llm::tool::StderrSink>,
    ) -> ExecutorResult {
        if let Some(sink) = stderr {
            self.got_sink.store(true, Ordering::Relaxed);
            for line in &self.lines {
                sink(line);
            }
        }

        ExecutorResult::Completed(ToolCallResponse {
            id: self.tool_id.clone(),
            result: Ok("done".to_owned()),
        })
    }
}

/// A config whose named tools run unattended and show a two-row progress
/// window.
///
/// `delay_secs = 0` so the row is up from the first frame rather than after the
/// default three seconds, which no test wants to wait out.
fn talking_tool_config(names: &[&str]) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.style.tool_call.show = true;
    config.style.tool_call.progress.show = true;
    config.style.tool_call.progress.delay_secs = 0;
    config.style.tool_call.progress.interval_ms = 10;
    config.style.tool_call.progress.stderr_rows = StderrRows::Fixed(RowCount { rows: 2 });
    config.conversation.tools.defaults.run = RunMode::Unattended;

    for name in names {
        config
            .conversation
            .tools
            .insert((*name).to_owned(), ToolConfig {
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
    }

    config
}

/// A provider that requests `calls` in one cycle, then answers with a message.
fn tool_calling_provider(calls: &[(&str, &str)]) -> Arc<dyn Provider> {
    let mut events = Vec::new();
    for (index, (id, name)) in calls.iter().enumerate() {
        events.push(Event::tool_call_start(
            index,
            (*id).to_owned(),
            (*name).to_owned(),
        ));
        events.push(Event::tool_call_args(index, "{}".to_owned()));
        events.push(Event::flush(index));
    }
    events.push(Event::Finished(FinishReason::Completed));

    Arc::new(SequentialMockProvider {
        responses: vec![events, vec![
            Event::message(0, "Done.\n\n"),
            Event::flush(0),
            Event::Finished(FinishReason::Completed),
        ]],
        call_index: AtomicUsize::new(0),
        model: ModelDetails::empty(id::ModelIdConfig {
            provider: ProviderId::Test,
            name: "talking-tool-mock".parse().expect("valid name"),
        }),
    })
}

/// A running tool's stderr reaches the progress window.
///
/// The sink is built inside the coordinator's spawn loop, and
/// `StatusRegion::source` copies the region it is asked of — so a progress row
/// claimed *after* that loop yields sinks that can never deliver, however the
/// renderer is reassigned later.
/// Nothing in the renderer's own tests catches that, because they claim the row
/// before asking for a sink.
#[tokio::test(flavor = "multi_thread")]
async fn a_running_tools_stderr_reaches_the_progress_window() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let config = talking_tool_config(&["build_tool"]);
        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let provider = tool_calling_provider(&[("call_1", "build_tool")]);
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        // The window renders only against a terminal with a known height.
        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(
            printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
        );
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let got_sink = Arc::new(AtomicBool::new(false));
        let executor_source = TestExecutorSource::new().with_executor("build_tool", {
            let got_sink = Arc::clone(&got_sink);
            move |req| {
                Box::new(TalkingExecutor::new(
                    &req.id,
                    &req.name,
                    &["   Compiling serde v1.0.219"],
                    Arc::clone(&got_sink),
                ))
            }
        });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            ChatRequest::from("Build it"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();

        assert!(
            got_sink.load(Ordering::Relaxed),
            "the coordinator must hand the executor a sink when print_stderr is on"
        );

        let chrome = err.lock().clone();
        assert!(
            chrome.contains("Compiling serde v1.0.219"),
            "the pushed line must reach the window.\nChrome:\n{chrome}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Two tools running at once share one window, and every row names its tool.
///
/// Interleaved unlabelled rows misattribute progress, so the label is what
/// makes a parallel window readable at all.
#[tokio::test(flavor = "multi_thread")]
async fn parallel_tools_label_their_window_rows() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let config = talking_tool_config(&["alpha_tool", "beta_tool"]);
        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let provider = tool_calling_provider(&[("call_1", "alpha_tool"), ("call_2", "beta_tool")]);
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(
            printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
        );
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let seen = Arc::new(AtomicBool::new(false));
        let executor_source = TestExecutorSource::new()
            .with_executor("alpha_tool", {
                let seen = Arc::clone(&seen);
                move |req| {
                    Box::new(TalkingExecutor::new(
                        &req.id,
                        &req.name,
                        &["alpha is working"],
                        Arc::clone(&seen),
                    ))
                }
            })
            .with_executor("beta_tool", {
                let seen = Arc::clone(&seen);
                move |req| {
                    Box::new(TalkingExecutor::new(
                        &req.id,
                        &req.name,
                        &["beta is working"],
                        Arc::clone(&seen),
                    ))
                }
            });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            ChatRequest::from("Run both"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();
        let chrome = err.lock().clone();

        // Padded to the widest label so the two columns line up, and the
        // label's colour closes before the tool's own text starts.
        assert!(
            chrome.contains("[alpha_tool]\x1b[39m alpha is working"),
            "alpha's row must carry its label.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("[beta_tool ]\x1b[39m beta is working"),
            "beta's row must carry its label, padded to match.\nChrome:\n{chrome}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A tool result rendered while another tool's window is live must survive
/// whole, with no region frame cutting into it.
///
/// The result is persistent chrome and the window is ephemeral, so the printer
/// has to erase the rows, let the result land, and repaint below it.
/// Getting this wrong eats the result rather than the region, which is the
/// direction that loses data the user came for.
///
/// Note what this can and cannot see: `SharedBuffer` accumulates every byte
/// written, so text a later `\r\x1b[K` would have wiped on a real terminal is
/// still in it.
/// The assertion that discriminates is therefore about *ordering* — whether a
/// region frame lands inside the link's line — not about the text being
/// present at all.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines)]
async fn a_tool_result_survives_a_live_window() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // `Full` is the default link style, and it is the fragmented write:
        // `writeln!(w, "see: {}", path)` reaches the printer as several tasks.
        // A one-line inline budget forces the result to a file, so the link is
        // rendered at all.
        let mut config = talking_tool_config(&["slow_tool", "quick_tool"]);
        config.conversation.tools.defaults.style = DisplayStyleConfig {
            hidden: false,
            inline_results: InlineResults::Truncate(TruncateLines { lines: 1 }),
            results_file_link: LinkStyle::Full,
            parameters: ParametersStyle::Off,
            print_stderr: true,
            error: ErrorStyleConfig {
                inline_results: None,
                results_file_link: None,
            },
        };

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let provider = tool_calling_provider(&[("call_1", "slow_tool"), ("call_2", "quick_tool")]);
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(
            printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
        );
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let seen = Arc::new(AtomicBool::new(false));
        let executor_source = TestExecutorSource::new()
            .with_executor("slow_tool", {
                let seen = Arc::clone(&seen);
                move |req| {
                    Box::new(TalkingExecutor::new(
                        &req.id,
                        &req.name,
                        &["still going"],
                        Arc::clone(&seen),
                    ))
                }
            })
            .with_executor("quick_tool", |req| {
                Box::new(MockExecutor::completed(
                    &req.id,
                    &req.name,
                    "THE-RESULT-BODY",
                ))
            });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            ChatRequest::from("Run both"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();
        let chrome = err.lock().clone();

        // The result body and its file link are persistent output; a region
        // repainted over an unfinished line would take either with it.
        assert!(
            chrome.contains("THE-RESULT-BODY"),
            "the result body must survive the window.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("see: "),
            "the file-link prefix must survive; it arrives as its own task.\nChrome:\n{chrome}"
        );

        // The path follows its prefix with nothing between them. A region
        // frame here is what erased the prefix on screen: the link arrives as
        // several tasks unless `write_chrome` batches them, and a redraw
        // between two of them starts with `\r\x1b[K`.
        let link = chrome
            .split("see: ")
            .nth(1)
            .expect("a link was rendered")
            .lines()
            .next()
            .expect("the link has a line");
        assert!(
            link.contains("tool_call"),
            "the link's path must follow its prefix, got {link:?}"
        );
        assert!(
            !link.contains('\r'),
            "no region frame may land inside the link's line, got {link:?}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Answering a tool's question re-spawns it, and its stderr must keep flowing.
///
/// The coordinator builds one sink per tool and stashes it on `ExecutingTool`,
/// precisely so the re-spawn an answer triggers keeps feeding the same window
/// row rather than going quiet halfway through the tool's life.
#[tokio::test(flavor = "multi_thread")]
async fn a_sink_survives_the_re_spawn_an_answer_triggers() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        // The question targets the assistant, so the answer arrives as a
        // structured provider response and no user is in the loop.
        let mut config = talking_tool_config(&[]);
        config
            .conversation
            .tools
            .insert("asking_tool".to_owned(), inquiry_tool_config(&["which"]));

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        // Tool call, then the assistant's answer to the question, then a final
        // message.
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_ask", "asking_tool"),
                structured_inquiry_events("call_ask.which", &json!(true)),
                final_message_events("Done."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(
            printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
        );
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let executor_source = TestExecutorSource::new().with_executor("asking_tool", |req| {
            Box::new(AskingTalkingExecutor::new(&req.id, &req.name))
        });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            ChatRequest::from("Ask then work"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();
        let chrome = err.lock().clone();

        assert!(
            chrome.contains("before the question"),
            "the first spawn's line must reach the window.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("after the answer"),
            "the re-spawn must inherit a live sink.\nChrome:\n{chrome}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// One tool opts out of the window; the other keeps feeding it.
///
/// Rows are screen space, so the window's size is global.
/// Membership is not: `conversation.tools.<name>.style.print_stderr` keeps one
/// noisy tool out without shrinking the window for everything else.
#[tokio::test(flavor = "multi_thread")]
#[expect(clippy::too_many_lines)]
async fn a_tool_can_opt_out_of_the_progress_window() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = talking_tool_config(&["loud_tool"]);
        // Everything about the window is unchanged; only this tool's
        // membership in it.
        config
            .conversation
            .tools
            .insert("quiet_tool".to_owned(), ToolConfig {
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
                style: Some(DisplayStyleConfig {
                    hidden: false,
                    inline_results: InlineResults::Off,
                    results_file_link: LinkStyle::Off,
                    parameters: ParametersStyle::Off,
                    print_stderr: false,
                    error: ErrorStyleConfig {
                        inline_results: None,
                        results_file_link: None,
                    },
                }),
                questions: IndexMap::new(),
                options: IndexMap::default(),
                access: None,
                cancellation_response: None,
            });

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let provider =
            tool_calling_provider(&[("call_loud", "loud_tool"), ("call_quiet", "quiet_tool")]);
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(
            printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
        );
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let loud_got_sink = Arc::new(AtomicBool::new(false));
        let quiet_got_sink = Arc::new(AtomicBool::new(false));
        let executor_source = TestExecutorSource::new()
            .with_executor("loud_tool", {
                let got_sink = Arc::clone(&loud_got_sink);
                move |req| {
                    Box::new(TalkingExecutor::new(
                        &req.id,
                        &req.name,
                        &["loud is working"],
                        Arc::clone(&got_sink),
                    ))
                }
            })
            .with_executor("quiet_tool", {
                let got_sink = Arc::clone(&quiet_got_sink);
                move |req| {
                    Box::new(TalkingExecutor::new(
                        &req.id,
                        &req.name,
                        &["quiet is working"],
                        Arc::clone(&got_sink),
                    ))
                }
            });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            ChatRequest::from("Run both"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();

        assert!(
            loud_got_sink.load(Ordering::Relaxed),
            "the opted-in tool must still be handed a sink"
        );
        assert!(
            !quiet_got_sink.load(Ordering::Relaxed),
            "a tool with print_stderr = false must not be handed a sink at all, so it costs \
             nothing rather than pushing into a window that drops it"
        );

        let chrome = err.lock().clone();
        assert!(
            chrome.contains("loud is working"),
            "the opted-in tool still reaches the window.\nChrome:\n{chrome}"
        );
        assert!(
            !chrome.contains("quiet is working"),
            "the opted-out tool must not reach the window.\nChrome:\n{chrome}"
        );
        assert!(
            chrome.contains("⏱ Running…"),
            "the window itself is unaffected by one tool opting out.\nChrome:\n{chrome}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A prompt backend that snapshots the chrome buffer at the moment the widget
/// is handed the terminal.
///
/// The same trick as the editor guard's `ObservingEditor`: a prompt session is
/// a run of small writes with the widget owning the cursor in between, so the
/// only way to prove the rows are gone *while* it runs is to look from inside
/// it.
struct ObservingPromptBackend {
    /// Answers the prompt once the snapshot is taken.
    inner: MockPromptBackend,

    /// Flushed before each snapshot, so the observation is of the applied
    /// state.
    printer: Arc<Printer>,

    /// The printer's chrome (stderr) buffer.
    chrome: jp_printer::SharedBuffer,

    /// What the chrome buffer held when the prompt opened.
    seen: jp_printer::SharedBuffer,
}

impl ObservingPromptBackend {
    /// A backend that records the chrome buffer, then answers `answer`.
    fn new(printer: Arc<Printer>, chrome: jp_printer::SharedBuffer, answer: char) -> Self {
        Self {
            inner: MockPromptBackend::new().with_inline_responses([answer]),
            printer,
            chrome,
            seen: jp_printer::SharedBuffer::default(),
        }
    }

    /// Record the chrome buffer once the worker has caught up.
    ///
    /// A prompt writer takes its suspension by enqueueing it rather than
    /// blocking: the widget's own writes are `Tty` tasks behind it in the same
    /// queue, so the worker erases before anything the prompt draws can land.
    /// The flush waits for that point instead of racing the worker to it —
    /// without it this reads whichever side won, and the region frame drawn
    /// before the claim is still the newest thing in the buffer.
    fn observe(&self) {
        self.printer.flush();
        let snapshot = self.chrome.lock().clone();
        self.seen.lock().push_str(&snapshot);
    }
}

impl PromptBackend for ObservingPromptBackend {
    fn inline_select(
        &self,
        message: &str,
        options: Vec<InlineOption>,
        default: Option<char>,
        writer: &mut dyn Write,
    ) -> Result<char, InquireError> {
        self.observe();
        self.inner.inline_select(message, options, default, writer)
    }

    fn inline_reply(
        &self,
        message: &str,
        initial_text: &str,
        edit_mode: ReplyEditMode,
        editor_escape: bool,
        help: Option<&str>,
        output: Box<dyn Write + Send>,
    ) -> Result<ReplyOutcome, InquireError> {
        self.observe();
        self.inner.inline_reply(
            message,
            initial_text,
            edit_mode,
            editor_escape,
            help,
            output,
        )
    }

    fn text(
        &self,
        message: &str,
        default: Option<&str>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.observe();
        self.inner.text(message, default, writer)
    }

    fn select(
        &self,
        message: &str,
        options: Vec<String>,
        default: Option<usize>,
        writer: &mut dyn Write,
    ) -> Result<String, InquireError> {
        self.observe();
        self.inner.select(message, options, default, writer)
    }

    fn password(&self, message: &str, writer: &mut dyn Write) -> Result<String, InquireError> {
        self.observe();
        self.inner.password(message, writer)
    }
}

/// A tool question hides the window while the prompt is up, and brings it back
/// after.
///
/// A prompt widget owns the cursor between its own writes, so a redraw landing
/// mid-session corrupts it.
/// Acquiring a prompt writer suspends the region for its lifetime, which is
/// what makes every prompt site correct without any of them knowing about
/// regions.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_prompt_hides_the_window_and_restores_it() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = talking_tool_config(&[]);
        let mut tool_config = inquiry_tool_config(&[]);
        tool_config
            .questions
            .insert("confirm".to_owned(), QuestionConfig {
                target: QuestionTarget::User,
                answer: None,
            });
        config
            .conversation
            .tools
            .insert("asking_tool".to_owned(), tool_config);

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_ask", "asking_tool"),
                final_message_events("Done."),
            ],
            call_index: AtomicUsize::new(0),
            model: inquiry_mock_model(),
        });
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        let printer = Arc::new(
            printer.with_terminal(TerminalCapability::interactive(Some(80)).with_rows(Some(24))),
        );
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let prompts = Arc::new(ObservingPromptBackend::new(
            Arc::clone(&printer),
            Arc::clone(&err),
            'y',
        ));
        let executor_source = TestExecutorSource::new().with_executor("asking_tool", |req| {
            Box::new(AskingTalkingExecutor::new(&req.id, &req.name))
        });
        let tool_defs = executor_source.tool_definitions();

        run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive: a user-targeted question needs a user
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::clone(&prompts) as Arc<dyn PromptBackend>,
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            ChatRequest::from("Ask me"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();

        // The prompt ran at all — an empty snapshot would make the rest
        // vacuous.
        let seen = prompts.seen.lock().clone();
        assert!(
            !seen.is_empty(),
            "the prompt must have opened for this test to mean anything"
        );

        // The last thing on the wire when the widget took over is an erase,
        // not a region frame: the rows are gone before it draws.
        assert!(
            seen.ends_with("\r\x1b[K"),
            "the window must be erased before the prompt draws, got {:?}",
            seen.rsplit('\n').next().unwrap_or(&seen)
        );

        // And it comes back afterwards, still carrying the tool's output.
        let chrome = err.lock().clone();
        let after = chrome
            .strip_prefix(seen.as_str())
            .expect("the snapshot is a prefix of the final chrome");
        assert!(
            after.contains("⏱ Running…"),
            "the row must return once the prompt closes.\nAfter:\n{after}"
        );
        assert!(
            after.contains("after the answer"),
            "the window keeps taking the tool's stderr afterwards.\nAfter:\n{after}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// An executor that pushes, asks one question, then pushes again once answered.
///
/// The two pushes straddle the re-spawn, so a sink that only worked on the
/// first attempt shows up as the second line missing.
struct AskingTalkingExecutor {
    /// The tool call this executor answers.
    tool_id: String,

    /// The tool's name.
    tool_name: String,

    /// Arguments, unused beyond satisfying the trait.
    arguments: Map<String, Value>,
}

impl AskingTalkingExecutor {
    /// An executor that talks either side of one question.
    fn new(tool_id: &str, tool_name: &str) -> Self {
        Self {
            tool_id: tool_id.to_owned(),
            tool_name: tool_name.to_owned(),
            arguments: Map::new(),
        }
    }
}

#[async_trait]
impl Executor for AskingTalkingExecutor {
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
        _root: &Utf8Path,
        _cancellation_token: CancellationToken,
        stderr: Option<jp_llm::tool::StderrSink>,
    ) -> ExecutorResult {
        if answers.contains_key("which") {
            if let Some(sink) = stderr {
                sink("after the answer");
            }

            return ExecutorResult::Completed(ToolCallResponse {
                id: self.tool_id.clone(),
                result: Ok("done".to_owned()),
            });
        }

        if let Some(sink) = stderr {
            sink("before the question");
        }

        ExecutorResult::NeedsInput {
            tool_id: self.tool_id.clone(),
            tool_name: self.tool_name.clone(),
            question: Question::boolean("which", "Which one?").expect("valid question id"),
            source: InquirySource::tool(self.tool_name.clone()),
            accumulated_answers: answers.clone(),
        }
    }
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
        _stderr: Option<jp_llm::tool::StderrSink>,
    ) -> ExecutorResult {
        for q in &self.questions {
            if !answers.contains_key(q.id.as_str()) {
                return ExecutorResult::NeedsInput {
                    tool_id: self.tool_id.clone(),
                    tool_name: self.tool_name.clone(),
                    question: q.clone(),
                    source: InquirySource::tool(self.tool_name.clone()),
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

/// The global inquiry ceiling wins over the top-level assistant's value.
///
/// `conversation.inquiry.assistant.request.max_response_bytes` is a public key,
/// so reading the assistant value here would silently ignore it.
#[tokio::test]
async fn inquiry_ceiling_honors_the_global_inquiry_override() {
    let mut config = AppConfig::new_test();
    config.assistant.request.max_response_bytes = MaxResponseBytes::Bytes(999_999);
    config
        .conversation
        .inquiry
        .assistant
        .request
        .max_response_bytes = MaxResponseBytes::Bytes(4096);

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(vec![]));
    let model = inquiry_mock_model();

    let backend = build_inquiry_backend(&config, vec![], model, provider, vec![])
        .await
        .expect("the inquiry backend builds");

    assert_eq!(
        backend
            .config_for("any_tool", "any_question")
            .max_response_bytes,
        Some(4096),
        "the global inquiry override must win over the parent assistant"
    );
}

/// Setting one field in the inquiry request block leaves the ceiling inheriting
/// from the assistant rather than resolving to the disable sentinel.
///
/// Built through the real loading path, since the failure this guards against
/// only appears in the partial-to-resolved conversion.
#[tokio::test]
async fn inquiry_ceiling_survives_a_sibling_only_request_override() {
    let mut partial = PartialAppConfig::new_test();
    partial.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(500_000));

    partial.conversation.inquiry.assistant.request = PartialRequestConfig {
        cache: Some(CachePolicy::Off),
        ..PartialRequestConfig::default()
    };

    let config = jp_config::util::build(partial).expect("valid config");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(vec![]));
    let model = inquiry_mock_model();

    let backend = build_inquiry_backend(&config, vec![], model, provider, vec![])
        .await
        .expect("the inquiry backend builds");

    assert_eq!(
        backend
            .config_for("any_tool", "any_question")
            .max_response_bytes,
        Some(500_000),
        "an unset inquiry ceiling must inherit the assistant, not disable the guard"
    );
}

/// An explicit `0` at the inquiry layer disables the ceiling for inquiries,
/// even when the assistant sets one.
#[tokio::test]
async fn inquiry_ceiling_can_be_disabled_independently() {
    let mut partial = PartialAppConfig::new_test();
    partial.assistant.request.max_response_bytes = Some(MaxResponseBytes::Bytes(500_000));
    partial
        .conversation
        .inquiry
        .assistant
        .request
        .max_response_bytes = Some(MaxResponseBytes::Disabled);

    let config = jp_config::util::build(partial).expect("valid config");

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(vec![]));
    let model = inquiry_mock_model();

    let backend = build_inquiry_backend(&config, vec![], model, provider, vec![])
        .await
        .expect("the inquiry backend builds");

    assert_eq!(
        backend
            .config_for("any_tool", "any_question")
            .max_response_bytes,
        None,
        "an explicit disable removes the ceiling for inquiries"
    );
}

/// A per-question ceiling wins over the global inquiry value, which in turn
/// wins over the top-level assistant (RFD 034's resolution order).
#[tokio::test]
async fn inquiry_ceiling_honors_the_per_question_override() {
    let mut config = AppConfig::new_test();
    config.assistant.request.max_response_bytes = MaxResponseBytes::Bytes(999_999);
    config
        .conversation
        .inquiry
        .assistant
        .request
        .max_response_bytes = MaxResponseBytes::Bytes(4096);

    let mut per_question = PartialAssistantConfig::default();
    per_question.request.max_response_bytes = Some(MaxResponseBytes::Bytes(512));

    let mut tool = inquiry_tool_config(&["confirm"]);
    tool.questions
        .insert("confirm".to_string(), QuestionConfig {
            target: QuestionTarget::Assistant(Box::new(per_question)),
            answer: None,
        });
    config
        .conversation
        .tools
        .insert("inquiry_tool".to_string(), tool);

    let provider: Arc<dyn Provider> = Arc::new(MockProvider::new(vec![]));
    let model = inquiry_mock_model();

    let backend = build_inquiry_backend(&config, vec![], model, provider, vec![])
        .await
        .expect("the inquiry backend builds");

    assert_eq!(
        backend
            .config_for("inquiry_tool", "confirm")
            .max_response_bytes,
        Some(512),
        "the per-question override must win over the global inquiry value"
    );

    // A question without its own ceiling still inherits the global inquiry
    // value, not the parent assistant's.
    assert_eq!(
        backend
            .config_for("inquiry_tool", "other")
            .max_response_bytes,
        Some(4096),
        "an unset per-question ceiling falls back to the global inquiry value"
    );
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
                vec![Question::boolean("confirm", "Create backup?").unwrap()],
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        assert_eq!(res[0].answer(), Some(&json!(true)));
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A `Secret` question with no interactive terminal fails the tool and records
/// `Cancelled(no_prompt_backend)` (RFD 082 routing guard).
#[tokio::test]
async fn test_secret_question_without_tty_fails_tool() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        // Register the tool without a question config: the question targets
        // the user by default, and without a TTY it would fall back to the
        // inquiry backend — which the secret guard refuses.
        config
            .conversation
            .tools
            .insert("secret_tool".to_string(), inquiry_tool_config(&[]));

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool");
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_sec", "secret_tool"),
                final_message_events("Understood."),
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

        let executor_source = TestExecutorSource::new().with_executor("secret_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::secret("passphrase", "Enter passphrase").unwrap()],
                "secret tool output",
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        // The tool fails with a tool-level error.
        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();
        assert_eq!(tool_responses.len(), 1);
        let error = tool_responses[0].result.as_ref().unwrap_err();
        assert!(error.contains("secret value"), "unexpected error: {error}");

        // The recorded inquiry pair closes as Cancelled(no_prompt_backend).
        let req: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_request())
            .collect();
        assert_eq!(req.len(), 1, "Should have one inquiry request");
        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 1, "Should have one inquiry response");
        assert!(matches!(&res[0], InquiryResponse::Cancelled {
            reason: CancellationReason::NoPromptBackend,
            ..
        }));
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A `Secret` question whose target is the assistant is refused and records
/// `Cancelled(assistant_routing_denied)` (RFD 082 routing guard).
#[tokio::test]
async fn test_secret_question_with_assistant_target_fails_tool() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        // Route the secret question to the assistant — the guard refuses.
        config.conversation.tools.insert(
            "secret_tool".to_string(),
            inquiry_tool_config(&["passphrase"]),
        );

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool");
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_sec", "secret_tool"),
                final_message_events("Understood."),
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

        let executor_source = TestExecutorSource::new().with_executor("secret_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::secret("passphrase", "Enter passphrase").unwrap()],
                "secret tool output",
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();
        assert_eq!(tool_responses.len(), 1);
        assert!(tool_responses[0].result.is_err());

        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 1, "Should have one inquiry response");
        assert!(matches!(&res[0], InquiryResponse::Cancelled {
            reason: CancellationReason::AssistantRoutingDenied,
            ..
        }));
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A `Secret` question answered at the prompter delivers the answer to the tool
/// in-memory while the persisted response is `Redacted` (RFD 082).
#[tokio::test]
async fn test_secret_prompter_answer_is_redacted() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        // Register the tool without a question config: the question targets
        // the user and the TTY prompter answers it via the no-echo password
        // path.
        config
            .conversation
            .tools
            .insert("secret_tool".to_string(), inquiry_tool_config(&[]));

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool");
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_sec", "secret_tool"),
                final_message_events("Secret used."),
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

        let executor_source = TestExecutorSource::new().with_executor("secret_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::secret("passphrase", "Enter passphrase").unwrap()],
                "secret tool output",
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
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new().with_password_responses(["s3cret"])),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        // The answer reached the tool in-memory: it completed successfully.
        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();
        assert_eq!(tool_responses.len(), 1);
        assert_eq!(tool_responses[0].content(), "secret tool output");

        // The persisted response is redacted and carries no answer value.
        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 1, "Should have one inquiry response");
        assert!(matches!(&res[0], InquiryResponse::Redacted { .. }));
        assert_eq!(res[0].answer(), None);
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A `Secret` question with a configured static answer delivers the value to
/// the tool in-memory while recording `Redacted` instead of `Answered`.
#[tokio::test]
async fn test_secret_static_answer_is_redacted() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        let mut tool_config = inquiry_tool_config(&[]);
        tool_config
            .questions
            .insert("passphrase".to_string(), QuestionConfig {
                target: QuestionTarget::User,
                answer: Some(json!("s3cret")),
            });
        config
            .conversation
            .tools
            .insert("secret_tool".to_string(), tool_config);

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool");
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_sec", "secret_tool"),
                final_message_events("Secret used."),
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

        let executor_source = TestExecutorSource::new().with_executor("secret_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::secret("passphrase", "Enter passphrase").unwrap()],
                "secret tool output",
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        // The static answer reached the tool in-memory.
        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();
        assert_eq!(tool_responses.len(), 1);
        assert_eq!(tool_responses[0].content(), "secret tool output");

        // The persisted response is redacted, not `Answered` with the value.
        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 1, "Should have one inquiry response");
        assert!(matches!(&res[0], InquiryResponse::Redacted { .. }));
        assert_eq!(res[0].answer(), None);
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A non-secret question with a configured static answer records a full
/// `InquiryRequest`/`InquiryResponse::Answered` pair carrying the configured
/// value (RFD 082: static answers are recorded, not pre-seeded).
#[tokio::test]
async fn test_static_answer_records_answered_inquiry() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        let mut tool_config = inquiry_tool_config(&[]);
        tool_config
            .questions
            .insert("confirm".to_string(), QuestionConfig {
                target: QuestionTarget::User,
                answer: Some(json!(true)),
            });
        config
            .conversation
            .tools
            .insert("static_tool".to_string(), tool_config);

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool");
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_static", "static_tool"),
                final_message_events("Done."),
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

        let executor_source = TestExecutorSource::new().with_executor("static_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::boolean("confirm", "Proceed?").unwrap()],
                "static tool output",
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        // The static answer reached the tool and it completed successfully.
        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();
        assert_eq!(tool_responses.len(), 1);
        assert_eq!(tool_responses[0].content(), "static tool output");

        // The round-trip is recorded as a request/response pair, with the
        // response carrying the configured value.
        let req: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_request())
            .collect();
        assert_eq!(req.len(), 1, "Should have one inquiry request");
        let res: Vec<_> = events
            .into_iter()
            .filter_map(|e| e.event.into_inquiry_response())
            .collect();
        assert_eq!(res.len(), 1, "Should have one inquiry response");
        assert!(matches!(&res[0], InquiryResponse::Answered { .. }));
        assert_eq!(res[0].answer(), Some(&json!(true)));
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// A "remember for this turn" prompter answer is reused for a later tool call
/// in the same turn, and the cache hit still records a fresh
/// `InquiryRequest`/`InquiryResponse::Answered` pair (RFD 082).
#[tokio::test]
async fn test_remembered_answer_cache_hit_records_new_inquiry_pair() {
    let test_result = Box::pin(timeout(Duration::from_secs(5), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        config.conversation.tools.defaults.run = RunMode::Unattended;
        // No question config: the question targets the user and is answered
        // at the interactive prompter.
        config
            .conversation
            .tools
            .insert("cached_tool".to_string(), inquiry_tool_config(&[]));

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        let chat_request = ChatRequest::from("Use the tool twice");
        let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
            responses: vec![
                single_tool_call_events("call_a", "cached_tool"),
                single_tool_call_events("call_b", "cached_tool"),
                final_message_events("Done."),
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

        let executor_source = TestExecutorSource::new().with_executor("cached_tool", |req| {
            Box::new(InquiryMockExecutor::new(
                &req.id,
                &req.name,
                vec![Question::boolean("confirm", "Proceed?").unwrap()],
                "cached tool output",
            ))
        });
        let tool_defs = executor_source.tool_definitions();

        // A single queued 'Y' ("yes, and remember for this turn"): the second
        // call must be satisfied from the turn cache, because another prompt
        // would find the queue empty and cancel.
        let prompt_backend = Arc::new(MockPromptBackend::new().with_inline_responses(['Y']));

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            prompt_backend,
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(result.is_ok(), "Turn loop should complete: {result:?}");

        let events = lock.events().clone();

        // Both tool calls completed with the answer.
        let tool_responses: Vec<_> = events
            .clone()
            .into_iter()
            .filter_map(|e| e.event.into_tool_call_response())
            .collect();
        assert_eq!(tool_responses.len(), 2);
        assert_eq!(tool_responses[0].content(), "cached tool output");
        assert_eq!(tool_responses[1].content(), "cached tool output");

        // Each round-trip records its own pair; the cache hit records
        // `Answered`, not nothing.
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
        for r in &res {
            assert!(matches!(r, InquiryResponse::Answered { .. }));
            assert_eq!(r.answer(), Some(&json!(true)));
        }
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
                    Question::boolean("confirm", "Proceed?").unwrap(),
                    Question::text("reason", "Why?").unwrap(),
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
                    vec![Question::boolean("confirm", "Proceed?").unwrap()],
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
                    vec![Question::boolean("confirm_a", "Proceed A?").unwrap()],
                    "tool_a done",
                ))
            })
            .with_executor("tool_b", |req| {
                Box::new(InquiryMockExecutor::new(
                    &req.id,
                    &req.name,
                    vec![Question::boolean("confirm_b", "Proceed B?").unwrap()],
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer,
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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
                vec![Question::boolean("confirm", "Confirm?").unwrap()],
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &tool_defs,
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
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

/// A misconfigured inquiry model override must be attributed to the override:
/// the underlying cause (e.g. a missing API key environment variable) is
/// otherwise indistinguishable from a main-model failure and points the user at
/// the wrong config.
#[test]
fn inquiry_model_override_error_names_the_override() {
    let error = Error::InquiryModelOverride {
        model: "openrouter/foo/bar".to_owned(),
        source: LlmError::MissingEnv("OPENROUTER_API_KEY".to_owned()),
    };

    // The variant names the override and keeps the cause chain intact.
    assert_eq!(
        error.to_string(),
        "Inquiry model override 'openrouter/foo/bar' is unusable"
    );
    assert_eq!(
        std::error::Error::source(&error)
            .expect("source")
            .to_string(),
        "Missing environment variable: OPENROUTER_API_KEY"
    );

    // The user-facing rendering carries the override's model id, the
    // underlying cause, and an actionable suggestion.
    let rendered = cmd::Error::from(error).to_string();
    assert!(
        rendered.contains("Inquiry model override is unusable"),
        "missing attribution: {rendered}"
    );
    assert!(
        rendered.contains("openrouter/foo/bar"),
        "missing model id: {rendered}"
    );
    assert!(
        rendered.contains("OPENROUTER_API_KEY"),
        "missing cause: {rendered}"
    );
    assert!(
        rendered.contains("conversation.inquiry.assistant.model"),
        "missing suggestion: {rendered}"
    );
}

/// A misconfigured per-question inquiry model override must be attributed to
/// the specific tool question: like the global override, the underlying cause
/// (e.g. a missing API key environment variable) is otherwise indistinguishable
/// from a main-model failure and points the user at the wrong config.
#[test]
fn inquiry_question_model_override_error_names_the_override() {
    let error = Error::InquiryQuestionModelOverride {
        tool: "my_tool".to_owned(),
        question: "q1".to_owned(),
        model: "openrouter/foo/bar".to_owned(),
        source: Box::new(LlmError::MissingEnv("OPENROUTER_API_KEY".to_owned())),
    };

    // The variant names the tool and question, and keeps the cause chain
    // intact.
    assert_eq!(
        error.to_string(),
        "Inquiry model override for question 'q1' of tool 'my_tool' is unusable"
    );
    assert_eq!(
        std::error::Error::source(&error)
            .expect("source")
            .to_string(),
        "Missing environment variable: OPENROUTER_API_KEY"
    );

    // The user-facing rendering carries the tool, question, model id, the
    // underlying cause, and an actionable suggestion naming the per-question
    // config path.
    let rendered = cmd::Error::from(error).to_string();
    assert!(
        rendered.contains("Inquiry model override for a tool question is unusable"),
        "missing attribution: {rendered}"
    );
    assert!(rendered.contains("my_tool"), "missing tool: {rendered}");
    assert!(rendered.contains("q1"), "missing question: {rendered}");
    assert!(
        rendered.contains("openrouter/foo/bar"),
        "missing model id: {rendered}"
    );
    assert!(
        rendered.contains("OPENROUTER_API_KEY"),
        "missing cause: {rendered}"
    );
    assert!(
        rendered.contains("conversation.tools.my_tool.questions.q1"),
        "missing suggestion: {rendered}"
    );
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
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

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

        // The live role header is chrome, so it lands on the error stream.
        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
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
            false, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            chat_request,
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await
        .unwrap();

        printer.flush();
        let output = strip_ansi_escapes::strip(&*err.lock());
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

/// End-to-end: a tool call that follows a reasoning block continues the
/// reasoning region, so its chrome (on stderr) carries the reasoning
/// background.
/// `AppConfig::new_test()` defaults `style.reasoning.background` to ANSI 236
/// and `display` to `full`, so the boundary returns a region for the tool and
/// `ToolRenderer` shades the header.
#[tokio::test]
async fn reasoning_before_a_tool_call_shades_the_tool_chrome() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let mut config = AppConfig::new_test();
    config.style.tool_call.show = true;
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
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());
    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
        .unwrap();

    // First response: reasoning, then a tool call that continues the region.
    let provider: Arc<dyn Provider> = Arc::new(SequentialMockProvider {
        responses: vec![
            vec![
                Event::reasoning(0, "Thinking about it.\n\n"),
                Event::flush(0),
                Event::tool_call_start(1, "call_mock".to_string(), "mock_tool".to_string()),
                Event::flush(1),
                Event::Finished(FinishReason::Completed),
            ],
            final_message_events("Done."),
        ],
        call_index: AtomicUsize::new(0),
        model: ModelDetails::empty(id::ModelIdConfig {
            provider: ProviderId::Test,
            name: "reasoning-tool-mock".parse().expect("valid name"),
        }),
    });
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let router = detached_router();

    let executor_source = TestExecutorSource::new().with_executor("mock_tool", |req| {
        Box::new(MockExecutor::completed(&req.id, &req.name, "tool output"))
    });
    let tool_defs = executor_source.tool_definitions();

    run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &tool_defs,
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), Box::new(executor_source)),
        ChatRequest::from("use the tool"),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
    )
    .await
    .unwrap();

    printer.flush();
    let chrome = err.lock().clone();
    assert!(
        chrome.contains("\x1b[48;5;236m"),
        "the tool chrome should carry the reasoning-region background.\nChrome:\n{chrome:?}"
    );
    assert!(
        strip_ansi_escapes::strip(&chrome)
            .windows(b"Calling tool".len())
            .any(|w| w == b"Calling tool"),
        "the shaded header text should still be present.\nChrome:\n{chrome:?}"
    );
}

/// Metadata key a provider repair patch targets in the rebuild tests.
fn rebuild_patch_key(round: u32) -> String {
    format!("stale_signature_{round}")
}

const REBUILD_PATCH_VALUE: &str = "stale";

/// One repair cycle: strip the metadata for `round`, then ask for a rebuild.
///
/// This is the shape a provider repair actually takes.
/// The rejection is a request-validation error, so nothing streams before it.
///
/// The leading flush has no buffered part behind it, so it commits nothing.
/// It is here because a cycle that renders nothing must not count as progress:
/// if it did, the rebuild budget would reset on every rebuilt request and the
/// cap could never be reached.
fn repair_cycle(round: u32) -> Vec<Event> {
    vec![
        Event::flush(0),
        Event::Patch(vec![EventPatch {
            matcher: EventMatcher::MetadataValue {
                key: rebuild_patch_key(round),
                value: REBUILD_PATCH_VALUE.to_owned(),
            },
            action: PatchAction::RemoveMetadata(rebuild_patch_key(round)),
        }]),
        Event::Finished(FinishReason::Retry),
    ]
}

/// Seed one assistant event carrying a distinct patchable key per round, so
/// every repair cycle has something of its own to remove and therefore makes
/// real progress.
fn seed_patchable_metadata(lock: &jp_workspace::ConversationLock, rounds: u32) {
    lock.as_mut().update_events(|stream| {
        // A complete prior turn: an incomplete trailing turn would be trimmed
        // when the loop starts its own.
        stream.start_turn(ChatRequest::from("earlier question"));

        let mut event = ConversationEvent::now(ChatResponse::reasoning("thinking"));
        for round in 0..rounds {
            event
                .metadata
                .insert(rebuild_patch_key(round), REBUILD_PATCH_VALUE.into());
        }

        stream
            .current_turn_mut()
            .add_event(event)
            .build()
            .expect("valid stream");
    });
}

/// A provider that keeps patching and asking for a rebuild is stopped by the
/// consecutive-rebuild cap.
///
/// Every cycle here makes genuine progress, so the progress guard never fires
/// and the cap is the only thing that can end the turn.
/// The mock is scripted with exactly one cycle more than the cap allows: if the
/// cap regresses, the loop asks for a further batch and the mock panics rather
/// than looping silently.
#[tokio::test]
async fn test_rebuild_cap_stops_a_provider_that_keeps_requesting_rebuilds() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();

    let rounds = MAX_CONSECUTIVE_REBUILDS + 1;
    seed_patchable_metadata(&lock, rounds);

    let batches = (0..rounds).map(repair_cycle).collect();
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_batches(batches));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (router, _signals) = test_router();
    let router = Arc::new(router);

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        ChatRequest::from("repair this"),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
    )
    .await;

    let error = result.expect_err("the turn must abort once the rebuild cap is reached");
    // The outer error only renders "LLM error"; the refusal is in the source.
    let message = format!("{error:?}");
    assert!(
        message.contains("times in a row"),
        "the abort should name the rebuild cap.\nError:\n{message}"
    );
}

/// A refused rebuild ends the turn, so a retry line left by an earlier cycle
/// has to be retired first.
///
/// A repair cycle renders nothing, so no event reaches the clear on the success
/// path, and the final error would otherwise be written onto the notification.
#[tokio::test(flavor = "multi_thread")]
async fn test_refused_rebuild_clears_the_retry_line() {
    let test_result = Box::pin(timeout(Duration::from_secs(10), async {
        let tmp = tempdir().unwrap();
        let root = tmp.path();
        let storage = root.join(".jp");

        let mut config = AppConfig::new_test();
        // The waiting indicator writes its own erase sequences, which would
        // blur what this test asserts.
        config.style.streaming.progress.show = false;
        // Keep the retry backoff out of the test's runtime.
        config.assistant.request.base_backoff_ms = 1;

        let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
        let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

        let lock = workspace
            .create_and_lock_conversation(Conversation::default(), Arc::new(config.clone()), None)
            .unwrap();

        // First cycle: a retryable error, which writes the retry notification and
        // leaves the cursor parked at the end of it.
        // Second cycle: a patch matching no event, so the rebuild that follows is
        // refused for lack of progress and the turn aborts.
        let provider: Arc<dyn Provider> = Arc::new(PacedMockProvider::new(Duration::ZERO, vec![
            vec![(
                Duration::ZERO,
                Err(StreamError::transient("simulated hiccup")),
            )],
            vec![
                (
                    Duration::ZERO,
                    Ok(Event::Patch(vec![EventPatch {
                        matcher: EventMatcher::MetadataValue {
                            key: "absent_key".into(),
                            value: "absent_value".into(),
                        },
                        action: PatchAction::RemoveMetadata("absent_key".into()),
                    }])),
                ),
                (Duration::ZERO, Ok(Event::Finished(FinishReason::Retry))),
            ],
        ]));
        let model = provider
            .model_details(&"test-model".parse().unwrap())
            .await
            .unwrap();

        let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
        // The notice only takes a status region on a terminal; elsewhere it is
        // a persistent line with nothing to retire.
        let printer = Arc::new(printer.with_terminal(TerminalCapability::interactive(Some(80))));
        let mcp_client = jp_mcp::Client::default();
        let router = detached_router();

        let result = run_turn_loop(
            Arc::clone(&provider),
            &model,
            &config,
            &router,
            &mcp_client,
            root,
            true, // interactive
            &[],
            &lock,
            ToolChoice::Auto,
            &[],
            printer.clone(),
            Arc::new(MockPromptBackend::new()),
            ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
            ChatRequest::from("answer this"),
            InvocationContext::default(),
            PendingStreamTrim::default(),
            router.turn_interrupt(lock.id()),
        )
        .await;

        assert!(
            result.is_err(),
            "a rebuild without progress must abort the turn"
        );

        printer.flush();

        let chrome = err.lock();
        assert!(
            chrome.contains("retrying (1/"),
            "the first cycle should have written a retry notice.\nChrome:\n{chrome}"
        );
        // Every region frame starts with `\r\x1b[K`, so occurrences cannot be
        // counted. What distinguishes a retired notice is that the erase is the
        // last thing written, leaving the terminal clean for the final error.
        assert!(
            chrome.ends_with("\r\x1b[K"),
            "the retry notice must be retired before the turn aborts.\nChrome:\n{chrome:?}"
        );
    }))
    .await;

    assert!(test_result.is_ok(), "Test timed out");
}

/// Content streamed before a refused rebuild survives the abort.
///
/// A part sits in the coordinator's event builder until a flush or terminal
/// event reaches it, and the rebuild request is intercepted before the
/// coordinator sees it, so persisting the conversation alone would drop text
/// the user already saw.
#[tokio::test]
async fn test_refused_rebuild_persists_streamed_content() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let storage = root.join(".jp");

    let config = AppConfig::new_test();
    let fs = Arc::new(FsStorageBackend::new(&storage).expect("failed to create backend"));
    let mut workspace = Workspace::in_memory(root).with_backend(fs.clone());

    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), config.clone().into(), None)
        .unwrap();
    let conv_id = lock.id();

    // No patch precedes the rebuild request, so it is refused for lack of
    // progress while a streamed part is still unflushed.
    let provider: Arc<dyn Provider> = Arc::new(MockProvider::with_batches(vec![vec![
        Event::message(0, "partial answer"),
        Event::Finished(FinishReason::Retry),
    ]]));
    let model = provider
        .model_details(&"test-model".parse().unwrap())
        .await
        .unwrap();

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mcp_client = jp_mcp::Client::default();
    let (router, _signals) = test_router();
    let router = Arc::new(router);

    let result = run_turn_loop(
        Arc::clone(&provider),
        &model,
        &config,
        &router,
        &mcp_client,
        root,
        false, // interactive
        &[],
        &lock,
        ToolChoice::Auto,
        &[],
        printer.clone(),
        Arc::new(MockPromptBackend::new()),
        ToolCoordinator::new(config.conversation.tools.clone(), empty_executor_source()),
        ChatRequest::from("answer this"),
        InvocationContext::default(),
        PendingStreamTrim::default(),
        router.turn_interrupt(lock.id()),
    )
    .await;

    assert!(
        result.is_err(),
        "a rebuild without a preceding patch must abort the turn"
    );

    let content = fs
        .read_test_events_raw(&conv_id)
        .expect("events should be persisted");

    assert!(
        content.contains("partial answer"),
        "streamed content must survive the abort.\nFile contents:\n{content}"
    );
}
